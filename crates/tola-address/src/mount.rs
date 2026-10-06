//! Browser URL mount applied outside the site-root address space.

use std::fmt;
use std::sync::Arc;

use percent_encoding::percent_decode_str;
use thiserror::Error;

use super::origin::SiteOrigin;
use super::{UrlPath, UrlPathError, portable_key_is_reserved};

/// Encoded browser path below which the complete site is mounted.
///
/// The root mount is represented by an empty string. Non-root mounts never
/// start or end with `/`; separators are always URL `/` separators.
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
        // Tola's own addresses live in the reserved namespace: development serves
        // `/_tola/preview` and `/_tola/hotreload.js`, and the published tree keeps its icons,
        // images, and code stylesheet under `_tola/`. A mount there would place site routes on
        // those addresses, so the first segment is refused by the same rule that refuses an output
        // path's root segment.
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

    /// Remove this deployment mount and recover the decoded site-root route.
    ///
    /// Returns `None` when the mount is not a prefix of `browser_path` or the remainder is not a
    /// valid site path. The mount's own prefix without a trailing slash names a file outside it.
    pub fn strip(&self, browser_path: &UrlPath) -> Option<UrlPath> {
        if self.is_root() {
            return Some(browser_path.clone());
        }
        // The mount is stored encoded and relative, while the browser path is decoded and
        // rooted, so the mount is decoded once and matched below the browser path's own `/`.
        let mount = percent_decode_str(&self.0).decode_utf8_lossy();
        let relative = browser_path
            .as_str()
            .strip_prefix('/')?
            .strip_prefix(mount.as_ref())?
            .strip_prefix('/')?;
        let mut route = String::with_capacity(relative.len() + 1);
        route.push('/');
        route.push_str(relative);
        UrlPath::from_decoded(&route).ok()
    }
}

/// The one browser rendering rule for a decoded site-root route.
///
/// The route is percent-encoded, the deployment mount is applied once, and a configured origin
/// makes the result absolute. Query and fragment are never part of the route and are appended by
/// the caller that owns them.
pub fn browser_url(route: &UrlPath, mount: &SiteUrlMount, origin: Option<&SiteOrigin>) -> String {
    let capacity = route.as_str().len()
        + mount.as_str().len()
        + 2
        + origin.map_or(0, |origin| origin.origin().len());
    let mut url = String::with_capacity(capacity);
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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::url_path;

    #[test]
    fn base_paths_parse_canonically() {
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
    fn invalid_base_paths_are_refused() {
        assert!(matches!(
            SiteUrlMount::from_base_path("/docs%2fadmin/"),
            Err(SiteUrlMountError::InvalidBasePath(
                UrlPathError::EncodedSeparator
            ))
        ));
        assert!(matches!(
            SiteUrlMount::from_base_path("/docs//admin/"),
            Err(SiteUrlMountError::InvalidBasePath(
                UrlPathError::EmptySegment
            ))
        ));
        assert!(matches!(
            SiteUrlMount::from_base_path("/docs"),
            Err(SiteUrlMountError::MissingTrailingSlash)
        ));
        for base_path in [
            "/docs/./admin/",
            "/docs/../admin/",
            "/docs/%2e%2e/admin/",
            "/docs/?preview=1",
            "/docs/#preview",
        ] {
            assert!(
                SiteUrlMount::from_base_path(base_path).is_err(),
                "accepted {base_path:?}"
            );
        }
    }

    #[test]
    fn reserved_root_mounts_are_refused() {
        for base_path in ["/_tola/", "/_tola/preview/", "/_TOLA/", "/%5Ftola/"] {
            assert!(
                matches!(
                    SiteUrlMount::from_base_path(base_path),
                    Err(SiteUrlMountError::ReservedRoot)
                ),
                "accepted {base_path:?}"
            );
        }
        // The syntax rule reports the missing slash before the reservation does.
        assert!(matches!(
            SiteUrlMount::from_base_path("/_tola"),
            Err(SiteUrlMountError::MissingTrailingSlash)
        ));
    }

    #[test]
    fn mounts_outside_the_reserved_root_are_accepted() {
        for base_path in ["/", "/_tola2/", "/docs/_tola/"] {
            assert!(
                SiteUrlMount::from_base_path(base_path).is_ok(),
                "refused {base_path:?}"
            );
        }
    }

    #[test]
    fn mount_applies_and_strips_routes() {
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
    fn stripped_routes_decode_the_mount() {
        let mount = SiteUrlMount::from_base_path("/文档/").unwrap();
        for site_path in [
            url_path("/styles/main.css"),
            url_path("/posts/"),
            url_path("/"),
        ] {
            let encoded = mount.browser_path(&site_path);
            assert_eq!(mount.strip(&url_path(&encoded)), Some(site_path));
        }
        assert_eq!(
            SiteUrlMount::from_base_path("/docs/")
                .unwrap()
                .strip(&url_path("/%64ocs/feed.xml")),
            Some(url_path("/feed.xml"))
        );
    }

    #[test]
    fn origin_makes_the_url_absolute() {
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
