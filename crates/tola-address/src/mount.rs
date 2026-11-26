//! Browser URL mount applied outside the site-root address space.

use std::fmt;
use std::sync::Arc;

use percent_encoding::percent_decode_str;
use thiserror::Error;

use super::origin::SiteOrigin;
use super::{UrlPath, UrlPathError, portable_key_is_reserved};

/// Encoded browser path below which the complete site is mounted.
///
/// Root mount is empty; non-root mounts never start or end with `/`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SiteUrlMount(Arc<str>);

#[derive(Debug, Error)]
pub enum SiteUrlMountError {
    #[error("`site.base-path` is not a valid URL path: {0}")]
    InvalidBasePath(#[from] UrlPathError),
    #[error("`site.base-path` must end with `/`")]
    MissingTrailingSlash,
    #[error("`site.base-path` must stay outside `_tola`, the namespace Tola reserves")]
    ReservedRoot,
}

impl SiteUrlMount {
    pub fn root() -> Self {
        Self::default()
    }

    /// Parse a canonical site base path.
    pub fn from_base_path(raw: &str) -> Result<Self, SiteUrlMountError> {
        if raw != "/" && !raw.ends_with('/') {
            return Err(SiteUrlMountError::MissingTrailingSlash);
        }
        let path = UrlPath::parse(raw)?;
        // `_tola/` is reserved for dev endpoints (`/_tola/preview`, `/_tola/hotreload.js`) and system assets.
        if portable_key_is_reserved(&path.url_collision_key()) {
            return Err(SiteUrlMountError::ReservedRoot);
        }
        Ok(Self(Arc::from(
            path.to_encoded().trim_matches('/').to_owned(),
        )))
    }

    /// Canonical rooted deployment path, including its trailing slash.
    pub fn base_path(&self) -> String {
        if self.is_root() {
            "/".to_owned()
        } else {
            format!("/{}/", self.as_str())
        }
    }

    /// Encoded relative mount, without leading or trailing `/`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Apply this deployment mount to a decoded site-root route.
    pub fn browser_path(&self, site_path: &UrlPath) -> String {
        browser_url(site_path, self, None)
    }

    /// Strip this deployment mount and recover the decoded site-root route.
    pub fn strip(&self, browser_path: &UrlPath) -> Option<UrlPath> {
        if self.is_root() {
            return Some(browser_path.clone());
        }
        // Stored mount is encoded and relative; `browser_path` is decoded and rooted.
        let mount = percent_decode_str(&self.0).decode_utf8_lossy();
        let rel = browser_path
            .as_str()
            .strip_prefix('/')?
            .strip_prefix(mount.as_ref())?
            .strip_prefix('/')?;
        let mut route = String::with_capacity(rel.len() + 1);
        route.push('/');
        route.push_str(rel);
        UrlPath::from_decoded(&route).ok()
    }
}

/// Format a browser URL for `route`, applying `mount` and optional `origin`.
pub fn browser_url(route: &UrlPath, mount: &SiteUrlMount, origin: Option<&SiteOrigin>) -> String {
    let cap =
        route.as_str().len() + mount.as_str().len() + 2 + origin.map_or(0, |o| o.origin().len());
    let mut url = String::with_capacity(cap);
    if let Some(origin) = origin {
        url.push_str(origin.origin());
    }
    if !mount.is_root() {
        url.push('/');
        url.push_str(mount.as_str());
    }
    route.push_encoded(&mut url);
    url
}

impl fmt::Display for SiteUrlMount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url_path(raw: &str) -> UrlPath {
        UrlPath::parse(raw).unwrap()
    }

    #[test]
    fn base_path_parse() {
        assert_eq!(
            SiteUrlMount::from_base_path("/").unwrap(),
            SiteUrlMount::root()
        );
        assert_eq!(
            SiteUrlMount::from_base_path("/docs/blog/")
                .unwrap()
                .as_str(),
            "docs/blog"
        );
        assert_eq!(
            SiteUrlMount::from_base_path("/文档/").unwrap().base_path(),
            "/%E6%96%87%E6%A1%A3/"
        );
        assert_eq!(
            SiteUrlMount::from_base_path("/%e6%96%87%e6%a1%a3/")
                .unwrap()
                .as_str(),
            "%E6%96%87%E6%A1%A3"
        );
    }

    #[test]
    fn reserved_root_error() {
        for raw in ["/_tola/", "/_tola/preview/", "/_TOLA/", "/%5Ftola/"] {
            assert!(
                matches!(
                    SiteUrlMount::from_base_path(raw),
                    Err(SiteUrlMountError::ReservedRoot)
                ),
                "accepted {raw:?}"
            );
        }
        assert!(matches!(
            SiteUrlMount::from_base_path("/_tola"),
            Err(SiteUrlMountError::MissingTrailingSlash)
        ));
    }

    #[test]
    fn mount_apply_strip() {
        let file = url_path("/_tola/reload.js");
        assert_eq!(SiteUrlMount::root().browser_path(&file), "/_tola/reload.js");
        assert_eq!(SiteUrlMount::root().strip(&file), Some(file));

        let mount = SiteUrlMount::from_base_path("/docs/blog/").unwrap();
        let page = url_path("/posts/rust/");
        assert_eq!(mount.browser_path(&page), "/docs/blog/posts/rust/");
        assert_eq!(mount.strip(&url_path("/docs/blog/posts/rust/")), Some(page));
        assert!(mount.strip(&url_path("/docs/blog2/posts/")).is_none());
    }

    #[test]
    fn browser_url_origin() {
        let mount = SiteUrlMount::from_base_path("/docs/").unwrap();
        let page = url_path("/posts/中文/");
        let origin = SiteOrigin::parse("https://example.com").unwrap();

        assert_eq!(
            browser_url(&page, &mount, None),
            "/docs/posts/%E4%B8%AD%E6%96%87/"
        );
        assert_eq!(
            browser_url(&page, &mount, Some(&origin)),
            "https://example.com/docs/posts/%E4%B8%AD%E6%96%87/"
        );
    }
}
