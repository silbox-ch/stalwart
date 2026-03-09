/*
 * SPDX-FileCopyrightText: 2025 Silbox CH
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Undelete/Recovery support — AGPL reimplementation.
//!
//! Stores lightweight metadata alongside `BlobOp::Undelete` markers so that
//! deleted emails can be listed and restored through the management API.

use store::{
    Deserialize, IterateParams, U32_LEN, U64_LEN, ValueKey,
    write::{AlignedBytes, Archive, BlobOp, ValueClass, key::DeserializeBigEndian, now},
};
use trc::AddContext;
use types::blob_hash::{BLOB_HASH_LEN, BlobHash};

/// Metadata stored with each `BlobOp::Undelete` marker.
///
/// This is a clean-room design: a flat struct with a human-readable
/// `description` field, intentionally different from the SEL tagged enum.
#[derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize, Debug, Clone)]
#[rkyv(derive(Debug))]
pub struct DeletedItem {
    /// Collection the item belonged to (e.g. `Collection::Email as u8`).
    pub collection: u8,
    /// Approximate size in bytes of the original blob.
    pub size: u32,
    /// Unix timestamp (seconds) when the item was deleted.
    pub deleted_at: u64,
    /// Human-readable summary, e.g. "From: alice@example.com, Subject: Hello".
    pub description: String,
}

/// A deleted blob ready for restore, with its hash and expiration.
pub struct DeletedBlob {
    pub hash: BlobHash,
    pub expires_at: u64,
    pub item: DeletedItem,
}

impl crate::Server {
    /// List all recoverable (non-expired) deleted items for an account.
    pub async fn list_deleted_items(
        &self,
        account_id: u32,
    ) -> trc::Result<Vec<DeletedBlob>> {
        let from_key = ValueKey {
            account_id,
            collection: 0,
            document_id: 0,
            class: ValueClass::Blob(BlobOp::Undelete {
                hash: BlobHash::default(),
                until: 0,
            }),
        };
        let to_key = ValueKey {
            account_id,
            collection: 0,
            document_id: u32::MAX,
            class: ValueClass::Blob(BlobOp::Undelete {
                hash: BlobHash::new_max(),
                until: u64::MAX,
            }),
        };

        let now = now();
        let mut results = Vec::new();

        self.store()
            .iterate(
                IterateParams::new(from_key, to_key).ascending(),
                |key, value| {
                    // Key layout: [UNDELETE_LINK(1)] [account_id(4)] [hash(32)] [until(8)]
                    let expires_at = key.deserialize_be_u64(key.len() - U64_LEN)?;
                    if expires_at > now {
                        let item =
                            <Archive<AlignedBytes> as Deserialize>::deserialize(value)
                                .and_then(|bytes| bytes.deserialize::<DeletedItem>())
                                .caused_by(trc::location!())?;

                        let hash = BlobHash::try_from_hash_slice(
                            key.get(U32_LEN + 1..U32_LEN + 1 + BLOB_HASH_LEN)
                                .ok_or_else(|| {
                                    trc::Error::corrupted_key(key, Some(value), trc::location!())
                                })?,
                        )
                        .map_err(|_| {
                            trc::Error::corrupted_key(key, Some(value), trc::location!())
                        })?;

                        results.push(DeletedBlob {
                            hash,
                            expires_at,
                            item,
                        });
                    }
                    Ok(true)
                },
            )
            .await
            .caused_by(trc::location!())?;

        Ok(results)
    }
}
