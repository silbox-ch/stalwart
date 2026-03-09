/*
 * SPDX-FileCopyrightText: 2025 Silbox CH
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Custom Branding support — AGPL reimplementation.
//!
//! Resolves a per-domain logo by checking the Domain principal's `picture`
//! field, falling back to `branding.logo-url` config, and caching the result.

use crate::manager::webadmin::Resource;
use directory::{QueryParams, Type, backend::internal::lookup::DirectoryStore};
use trc::AddContext;
use utils::HttpLimitResponse;

const MAX_IMAGE_SIZE: usize = 1024 * 1024;

impl crate::Server {
    /// Resolve the logo for a given domain (from Host header).
    ///
    /// Resolution order:
    /// 1. In-memory cache (`self.inner.data.logos`)
    /// 2. Domain principal's `picture` field (if it's an HTTP URL → download)
    /// 3. Global `branding.logo-url` config
    /// 4. `None` (caller falls back to webadmin bundled logo)
    pub async fn resolve_logo(&self, domain: &str) -> trc::Result<Option<Resource<Vec<u8>>>> {
        let domain = psl::domain_str(domain).unwrap_or(domain);

        // 1. Check cache
        let cached = { self.inner.data.logos.lock().get(domain).cloned() };
        if let Some(logo) = cached {
            return Ok(logo);
        }

        // 2. Look up Domain principal → picture field
        let logo_url = if let Some(mut principal) = self
            .store()
            .query(QueryParams::name(domain).with_return_member_of(false))
            .await
            .caused_by(trc::location!())?
            .filter(|p| p.typ() == Type::Domain)
        {
            if let Some(logo) = principal.picture_mut().filter(|l| l.starts_with("http")) {
                std::mem::take(logo).into()
            } else {
                self.core.jmap.branding_logo_url.clone()
            }
        } else {
            self.core.jmap.branding_logo_url.clone()
        };

        // 3. Download if URL found
        let mut logo = None;
        if let Some(logo_url) = logo_url {
            let response = reqwest::get(logo_url.as_str()).await.map_err(|err| {
                trc::ResourceEvent::DownloadExternal
                    .into_err()
                    .details("Failed to download logo")
                    .reason(err)
            })?;

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|ct| ct.to_str().ok())
                .unwrap_or("image/svg+xml")
                .to_string();

            let contents = response
                .bytes_with_limit(MAX_IMAGE_SIZE)
                .await
                .map_err(|err| {
                    trc::ResourceEvent::DownloadExternal
                        .into_err()
                        .details("Failed to download logo")
                        .reason(err)
                })?
                .ok_or_else(|| {
                    trc::ResourceEvent::DownloadExternal
                        .into_err()
                        .details("Download exceeded maximum size")
                })?;

            logo = Resource::new(content_type, contents).into();
        }

        // 4. Cache the result (even if None, to avoid repeated lookups)
        self.inner
            .data
            .logos
            .lock()
            .insert(domain.to_string(), logo.clone());

        Ok(logo)
    }
}
