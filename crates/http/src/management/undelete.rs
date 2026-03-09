/*
 * SPDX-FileCopyrightText: 2025 Silbox CH
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Undelete/Recovery management API — AGPL reimplementation.
//!
//! GET  /api/store/undelete/{account}  — list deleted items
//! POST /api/store/undelete/{account}  — restore deleted items by hash

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use common::Server;
use directory::backend::internal::manage::ManageDirectory;
use email::{
    mailbox::INBOX_ID,
    message::ingest::{EmailIngest, IngestEmail, IngestSource},
};
use http_proto::{request::decode_path_element, *};
use hyper::Method;
use mail_parser::{DateTime, MessageParser};
use serde_json::json;
use std::future::Future;
use store::write::{BatchBuilder, BlobLink, BlobOp};
use trc::AddContext;
use types::blob_hash::BlobHash;
use utils::url_params::UrlParams;

/// Request body for restoring a single deleted item.
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct RestoreRequest {
    hash: String,
    #[serde(default)]
    cancel_deletion: bool,
}

/// Response for each restore attempt.
#[derive(serde::Serialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
enum RestoreResponse {
    Success,
    NotFound,
    Error { reason: String },
}

pub trait UndeleteHandler: Sync + Send {
    fn handle_undelete_request(
        &self,
        req: &HttpRequest,
        path: Vec<&str>,
        body: Option<Vec<u8>>,
        session: &HttpSessionData,
    ) -> impl Future<Output = trc::Result<HttpResponse>> + Send;
}

impl UndeleteHandler for Server {
    async fn handle_undelete_request(
        &self,
        req: &HttpRequest,
        path: Vec<&str>,
        body: Option<Vec<u8>>,
        session: &HttpSessionData,
    ) -> trc::Result<HttpResponse> {
        match (path.get(2).copied(), req.method()) {
            // GET /api/store/undelete/{account} — list deleted items
            (Some(account_name), &Method::GET) => {
                let account_name = decode_path_element(account_name);
                let account_id = self
                    .core
                    .storage
                    .data
                    .get_principal_id(account_name.as_ref())
                    .await?
                    .ok_or_else(|| trc::ResourceEvent::NotFound.into_err())?;

                let mut deleted = self.list_deleted_items(account_id).await?;

                // Pagination
                let params = UrlParams::new(req.uri().query());
                let limit = params.parse::<usize>("limit").unwrap_or_default();
                let page = params.parse::<usize>("page").unwrap_or(1);
                let offset = page.saturating_sub(1) * limit;

                // Sort ascending by deleted_at
                let total = deleted.len();
                deleted.sort_by_key(|d| d.item.deleted_at);

                // Build page of results
                let mut items = Vec::with_capacity(if limit > 0 { limit } else { total });
                for (i, blob) in deleted.into_iter().enumerate() {
                    if i < offset {
                        continue;
                    }
                    items.push(json!({
                        "hash": URL_SAFE_NO_PAD.encode(blob.hash.as_slice()),
                        "collection": collection_name(blob.item.collection),
                        "size": blob.item.size,
                        "deletedAt": DateTime::from_timestamp(blob.item.deleted_at as i64).to_rfc3339(),
                        "expiresAt": DateTime::from_timestamp(blob.expires_at as i64).to_rfc3339(),
                        "description": blob.item.description,
                    }));
                    if limit > 0 && items.len() >= limit {
                        break;
                    }
                }

                Ok(JsonResponse::new(json!({
                    "data": {
                        "items": items,
                        "total": total,
                    },
                }))
                .into_http_response())
            }

            // POST /api/store/undelete/{account} — restore deleted items
            (Some(account_name), &Method::POST) => {
                let account_name = decode_path_element(account_name);
                let account_id = self
                    .core
                    .storage
                    .data
                    .get_principal_id(account_name.as_ref())
                    .await?
                    .ok_or_else(|| trc::ResourceEvent::NotFound.into_err())?;

                // Parse request body
                let requests: Vec<RestoreRequest> =
                    serde_json::from_slice(body.as_deref().unwrap_or(b"[]"))
                        .map_err(|err| trc::ResourceEvent::BadParameters.into_err().reason(err))?;

                if requests.is_empty() {
                    // Empty body → restore all
                    let deleted = self.list_deleted_items(account_id).await?;
                    return self
                        .restore_all(account_id, deleted, session.session_id)
                        .await;
                }

                let access_token = self
                    .get_access_token(account_id)
                    .await
                    .caused_by(trc::location!())?;

                let mut results = Vec::with_capacity(requests.len());
                let mut batch = BatchBuilder::new();
                batch.with_account_id(account_id);

                for request in requests {
                    // Decode hash from base64
                    let hash = match URL_SAFE_NO_PAD
                        .decode(request.hash.as_bytes())
                        .ok()
                        .and_then(|bytes| BlobHash::try_from_hash_slice(&bytes).ok())
                    {
                        Some(hash) => hash,
                        None => {
                            results.push(RestoreResponse::Error {
                                reason: "Invalid hash".to_string(),
                            });
                            continue;
                        }
                    };

                    // Retrieve blob
                    match self
                        .blob_store()
                        .get_blob(hash.as_slice(), 0..usize::MAX)
                        .await?
                    {
                        Some(bytes) => {
                            match self
                                .email_ingest(IngestEmail {
                                    raw_message: &bytes,
                                    message: MessageParser::new().parse(&bytes),
                                    blob_hash: Some(&hash),
                                    access_token: access_token.as_ref(),
                                    mailbox_ids: vec![INBOX_ID],
                                    keywords: vec![],
                                    received_at: None,
                                    source: IngestSource::Restore,
                                    session_id: session.session_id,
                                })
                                .await
                            {
                                Ok(_) => {
                                    results.push(RestoreResponse::Success);
                                    if request.cancel_deletion {
                                        // Find and clear the undelete markers
                                        let deleted =
                                            self.list_deleted_items(account_id).await?;
                                        for blob in deleted {
                                            if blob.hash == hash {
                                                batch
                                                    .clear(BlobOp::Link {
                                                        hash: blob.hash.clone(),
                                                        to: BlobLink::Temporary {
                                                            until: blob.expires_at,
                                                        },
                                                    })
                                                    .clear(BlobOp::Undelete {
                                                        hash: blob.hash,
                                                        until: blob.expires_at,
                                                    });
                                                break;
                                            }
                                        }
                                    }
                                }
                                Err(mut err)
                                    if err.matches(trc::EventType::MessageIngest(
                                        trc::MessageIngestEvent::Error,
                                    )) =>
                                {
                                    results.push(RestoreResponse::Error {
                                        reason: err
                                            .take_value(trc::Key::Reason)
                                            .and_then(|v| v.into_string())
                                            .unwrap_or_default()
                                            .to_string(),
                                    });
                                }
                                Err(err) => {
                                    return Err(err.caused_by(trc::location!()));
                                }
                            }
                        }
                        None => {
                            results.push(RestoreResponse::NotFound);
                        }
                    }
                }

                // Commit batch (undelete marker cleanup)
                if !batch.is_empty() {
                    self.core
                        .storage
                        .data
                        .write(batch.build_all())
                        .await
                        .caused_by(trc::location!())?;
                }

                Ok(JsonResponse::new(json!({
                    "data": results,
                }))
                .into_http_response())
            }

            _ => Err(trc::ResourceEvent::NotFound.into_err()),
        }
    }
}

impl Server {
    /// Restore all deleted items for an account (empty POST body).
    async fn restore_all(
        &self,
        account_id: u32,
        deleted: Vec<common::undelete::DeletedBlob>,
        session_id: u64,
    ) -> trc::Result<HttpResponse> {
        let access_token = self
            .get_access_token(account_id)
            .await
            .caused_by(trc::location!())?;

        let mut results = Vec::with_capacity(deleted.len());
        let mut batch = BatchBuilder::new();
        batch.with_account_id(account_id);

        for blob in deleted {
            // Only restore emails for now
            if blob.item.collection != types::collection::Collection::Email as u8 {
                results.push(RestoreResponse::Error {
                    reason: "Unsupported collection".to_string(),
                });
                continue;
            }

            match self
                .blob_store()
                .get_blob(blob.hash.as_slice(), 0..usize::MAX)
                .await?
            {
                Some(bytes) => {
                    match self
                        .email_ingest(IngestEmail {
                            raw_message: &bytes,
                            message: MessageParser::new().parse(&bytes),
                            blob_hash: Some(&blob.hash),
                            access_token: access_token.as_ref(),
                            mailbox_ids: vec![INBOX_ID],
                            keywords: vec![],
                            received_at: Some(blob.item.deleted_at),
                            source: IngestSource::Restore,
                            session_id,
                        })
                        .await
                    {
                        Ok(_) => {
                            results.push(RestoreResponse::Success);
                            batch
                                .clear(BlobOp::Link {
                                    hash: blob.hash.clone(),
                                    to: BlobLink::Temporary {
                                        until: blob.expires_at,
                                    },
                                })
                                .clear(BlobOp::Undelete {
                                    hash: blob.hash,
                                    until: blob.expires_at,
                                });
                        }
                        Err(mut err)
                            if err.matches(trc::EventType::MessageIngest(
                                trc::MessageIngestEvent::Error,
                            )) =>
                        {
                            results.push(RestoreResponse::Error {
                                reason: err
                                    .take_value(trc::Key::Reason)
                                    .and_then(|v| v.into_string())
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                        Err(err) => {
                            return Err(err.caused_by(trc::location!()));
                        }
                    }
                }
                None => {
                    results.push(RestoreResponse::NotFound);
                }
            }
        }

        // Commit batch (undelete marker cleanup)
        if !batch.is_empty() {
            self.core
                .storage
                .data
                .write(batch.build_all())
                .await
                .caused_by(trc::location!())?;
        }

        Ok(JsonResponse::new(json!({
            "data": results,
        }))
        .into_http_response())
    }
}

fn collection_name(collection: u8) -> &'static str {
    use types::collection::Collection;
    match collection {
        c if c == Collection::Email as u8 => "email",
        c if c == Collection::FileNode as u8 => "file",
        c if c == Collection::CalendarEvent as u8 => "calendar",
        c if c == Collection::ContactCard as u8 => "contact",
        _ => "unknown",
    }
}
