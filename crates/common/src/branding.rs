/*
 * SPDX-FileCopyrightText: 2025 Silbox CH
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Custom Branding support — AGPL reimplementation.
//!
//! Resolves per-domain branding assets (logo, favicons, manifest) with
//! multi-tenant fallback: Domain principal → Tenant principal → global config.
//! Strictly superior to the enterprise SEL version which only handles `/logo.svg`.

use crate::manager::webadmin::Resource;
use directory::{QueryParams, Type, backend::internal::lookup::DirectoryStore};
use trc::AddContext;
use utils::HttpLimitResponse;

const MAX_ASSET_SIZE: usize = 1024 * 1024;

/// Branding asset types served by the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrandingAsset {
    Logo,
    FaviconIco,
    FaviconSvg,
    AppleTouchIcon,
    Manifest,
}

impl BrandingAsset {
    /// Parse from the first path segment of an HTTP request.
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "logo.svg" => Some(Self::Logo),
            "favicon.ico" => Some(Self::FaviconIco),
            "favicon.svg" => Some(Self::FaviconSvg),
            "apple-touch-icon.png" => Some(Self::AppleTouchIcon),
            "manifest.json" => Some(Self::Manifest),
            // Webadmin bundles use hashed filenames (e.g. favicon-cfb706b7132e0fd2.ico)
            _ if path.starts_with("favicon-") && path.ends_with(".ico") => Some(Self::FaviconIco),
            _ => None,
        }
    }

    /// Filename appended to the assets base URL for download.
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Logo => "logo.svg",
            Self::FaviconIco => "favicon.ico",
            Self::FaviconSvg => "favicon.svg",
            Self::AppleTouchIcon => "apple-touch-icon.png",
            Self::Manifest => "manifest.json",
        }
    }

    /// Default Content-Type when the remote server doesn't provide one.
    pub fn default_content_type(&self) -> &'static str {
        match self {
            Self::Logo | Self::FaviconSvg => "image/svg+xml",
            Self::FaviconIco => "image/x-icon",
            Self::AppleTouchIcon => "image/png",
            Self::Manifest => "application/manifest+json",
        }
    }

    /// Short suffix for the composite cache key.
    fn cache_suffix(&self) -> &'static str {
        match self {
            Self::Logo => ":logo",
            Self::FaviconIco => ":fav-ico",
            Self::FaviconSvg => ":fav-svg",
            Self::AppleTouchIcon => ":apple",
            Self::Manifest => ":manifest",
        }
    }
}

impl crate::Server {
    /// Resolve a branding asset for a given domain (from Host header).
    ///
    /// Resolution order:
    /// 1. In-memory cache (`self.inner.data.logos`) keyed by `"{domain}:{asset}"`
    /// 2. For Logo only: Domain principal's `picture` field → Tenant `picture` → `branding.logo-url`
    /// 3. Assets base URL: Domain `urls()[0]` → Tenant `urls()[0]` → `branding.assets-url`
    /// 4. Download `{base_url}/{asset_filename}`
    /// 5. `None` (caller falls back to webadmin bundled asset)
    pub async fn resolve_branding_asset(
        &self,
        domain: &str,
        asset: BrandingAsset,
    ) -> trc::Result<Option<Resource<Vec<u8>>>> {
        let domain = psl::domain_str(domain).unwrap_or(domain);
        let cache_key = format!("{domain}{}", asset.cache_suffix());

        // 1. Check cache
        let cached = { self.inner.data.logos.lock().get(&cache_key).cloned() };
        if let Some(result) = cached {
            return Ok(result);
        }

        // 2. For Logo: try the picture field first (backward compat)
        if asset == BrandingAsset::Logo {
            if let Some(url) = self.resolve_logo_url(domain).await? {
                let resource = self.download_asset(&url, asset.default_content_type()).await;
                match resource {
                    Ok(r) => {
                        let result = Some(r);
                        self.inner
                            .data
                            .logos
                            .lock()
                            .insert(cache_key, result.clone());
                        return Ok(result);
                    }
                    Err(err) => {
                        trc::error!(err);
                        // Fall through to assets-url
                    }
                }
            }
        }

        // 3. Resolve assets base URL: Domain urls() → Tenant urls() → config
        let result = if let Some(base_url) = self.resolve_assets_base_url(domain).await? {
            let url = format!("{}{}", base_url, asset.filename());
            match self.download_asset(&url, asset.default_content_type()).await {
                Ok(resource) => Some(resource),
                Err(err) => {
                    trc::error!(err);
                    None
                }
            }
        } else {
            None
        };

        // 4. Cache the result (even if None, to avoid repeated lookups)
        self.inner
            .data
            .logos
            .lock()
            .insert(cache_key, result.clone());

        Ok(result)
    }

    /// Resolve logo URL from the `picture` field on Domain → Tenant → config.
    async fn resolve_logo_url(&self, domain: &str) -> trc::Result<Option<String>> {
        // Query Domain principal
        if let Some(mut principal) = self
            .store()
            .query(QueryParams::name(domain).with_return_member_of(false))
            .await
            .caused_by(trc::location!())?
            .filter(|p| p.typ() == Type::Domain)
        {
            // Domain picture field
            if let Some(logo) = principal.picture_mut().filter(|l| l.starts_with("http")) {
                return Ok(Some(std::mem::take(logo)));
            }

            // Tenant picture fallback
            if let Some(tenant_id) = principal.tenant() {
                if let Some(logo) = self
                    .store()
                    .query(QueryParams::id(tenant_id).with_return_member_of(false))
                    .await
                    .caused_by(trc::location!())?
                    .and_then(|mut p| p.picture_mut().map(std::mem::take))
                    .filter(|l| l.starts_with("http"))
                {
                    return Ok(Some(logo));
                }
            }
        }

        // Global config fallback
        Ok(self.core.jmap.branding_logo_url.clone())
    }

    /// Resolve the assets base URL for a domain.
    /// Domain urls() → Tenant urls() → config branding.assets-url
    async fn resolve_assets_base_url(&self, domain: &str) -> trc::Result<Option<String>> {
        // Query Domain principal
        if let Some(principal) = self
            .store()
            .query(QueryParams::name(domain).with_return_member_of(false))
            .await
            .caused_by(trc::location!())?
            .filter(|p| p.typ() == Type::Domain)
        {
            // First URL on the Domain principal (used as assets base)
            if let Some(url) = principal.urls().next().filter(|u| u.starts_with("http")) {
                let url = url.clone();
                return Ok(Some(if url.ends_with('/') {
                    url
                } else {
                    format!("{url}/")
                }));
            }

            // Tenant urls fallback
            if let Some(tenant_id) = principal.tenant() {
                if let Some(url) = self
                    .store()
                    .query(QueryParams::id(tenant_id).with_return_member_of(false))
                    .await
                    .caused_by(trc::location!())?
                    .and_then(|p| p.urls().next().filter(|u| u.starts_with("http")).cloned())
                {
                    return Ok(Some(if url.ends_with('/') {
                        url
                    } else {
                        format!("{url}/")
                    }));
                }
            }
        }

        // Global config fallback
        Ok(self.core.jmap.branding_assets_url.clone())
    }

    /// Download an asset from a URL with size limit.
    async fn download_asset(
        &self,
        url: &str,
        default_content_type: &str,
    ) -> trc::Result<Resource<Vec<u8>>> {
        let response = reqwest::get(url).await.map_err(|err| {
            trc::ResourceEvent::DownloadExternal
                .into_err()
                .details("Failed to download branding asset")
                .reason(err)
        })?;

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|ct| ct.to_str().ok())
            .unwrap_or(default_content_type)
            .to_string();

        let contents = response
            .bytes_with_limit(MAX_ASSET_SIZE)
            .await
            .map_err(|err| {
                trc::ResourceEvent::DownloadExternal
                    .into_err()
                    .details("Failed to read branding asset")
                    .reason(err)
            })?
            .ok_or_else(|| {
                trc::ResourceEvent::DownloadExternal
                    .into_err()
                    .details("Branding asset exceeded maximum size")
            })?;

        Ok(Resource::new(content_type, contents))
    }
}
