//! Link and URL processor (Indexed -> Indexed).
//!
//! Processes link and heading attributes:
//! - Link family: href attributes (absolute, relative, fragment, external)
//! - Heading family: id attribute slugification
//!
//! # Link Resolution
//!
//! Links are resolved based on their syntax using [`LinkKind`]:
//!
//! | LinkKind | Example | Result |
//! |----------|---------|--------|
//! | `External` | `https://...` | Preserved as-is |
//! | `Fragment` | `#section` | Slugified anchor |
//! | `SiteRoot` | `/about` | Prefixed and slugified |
//! | `FileRelative` | `./img.png` | Adjusted for output structure |

use anyhow::Result;
use tola_vdom::prelude::*;

use crate::compiler::family::{Indexed, TolaSite::FamilyKind};
use crate::compiler::page::PageRoute;
use crate::config::SiteConfig;
use crate::core::{LinkKind, UrlPath};
use crate::utils::path::route::split_path_fragment;
use crate::utils::path::slug::{slugify_fragment, slugify_path};

// =============================================================================
// VDOM Transform
// =============================================================================

/// Processes link href and heading id attributes in Indexed VDOM
pub struct LinkTransform<'a> {
    config: &'a SiteConfig,
    route: &'a PageRoute,
}

impl<'a> LinkTransform<'a> {
    pub fn new(config: &'a SiteConfig, route: &'a PageRoute) -> Self {
        Self { config, route }
    }

    /// Process a link href and keep indexed family data in sync.
    fn process_href(&self, elem: &mut Element<Indexed>) {
        let Some(value) = elem.get_attr("href").map(str::to_string) else {
            return;
        };

        if let Ok(processed) = process_link_value(&value, self.config, self.route) {
            elem.set_attr("href", processed.clone());
            if let Some(data) = ExtractFamily::<LinkFamily>::get_mut(&mut elem.ext) {
                data.set_href(Some(processed));
            }
        }
    }

    /// Slugify a heading id and keep indexed family data in sync.
    fn process_heading_id(&self, elem: &mut Element<Indexed>) {
        let Some(id) = elem.get_attr("id").map(str::to_string) else {
            return;
        };

        let slugged = slugify_fragment(&id, &self.config.build.slug);
        elem.set_attr("id", slugged.clone());
        if let Some(data) = ExtractFamily::<HeadingFamily>::get_mut(&mut elem.ext) {
            data.set_id(Some(slugged));
        }
    }
}

impl Transform<Indexed> for LinkTransform<'_> {
    type To = Indexed;

    fn transform(self, mut doc: Document<Indexed>) -> Document<Indexed> {
        // Link href
        doc.modify_by::<FamilyKind::Link, _>(|elem| {
            self.process_href(elem);
        });

        // Heading id slugify
        doc.modify_by::<FamilyKind::Heading, _>(|elem| {
            self.process_heading_id(elem);
        });

        doc
    }
}

// =============================================================================
// Link Processing Logic
// =============================================================================

/// Resolve a link to its final URL string
///
/// This is the main entry point for link resolution. It classifies the link
/// syntactically using [`LinkKind`], then resolves it based on context
///
/// # Link Types
///
/// - External URLs (https://, mailto:, etc.) -> preserved as-is
/// - Fragment anchors (#section) -> slugified
/// - Site-root links (/about) -> prefixed and slugified
/// - File-relative (./image.png) -> adjusted for output structure
pub fn resolve_link(value: &str, config: &SiteConfig, route: &PageRoute) -> Result<String> {
    if value.is_empty() {
        anyhow::bail!("empty link URL found");
    }

    let url = match LinkKind::parse(value) {
        LinkKind::External(url) => url.to_string(),

        LinkKind::Fragment(anchor) => {
            format!("#{}", slugify_fragment(anchor, &config.build.slug))
        }

        LinkKind::SiteRoot(path) => resolve_site_root(path, config)?,

        LinkKind::FileRelative(path) => resolve_file_relative(path, route),
    };

    Ok(url)
}

/// Process a link value (href or src attribute)
///
/// Alias for [`resolve_link`] for clarity at call sites
#[inline]
pub fn process_link_value(value: &str, config: &SiteConfig, route: &PageRoute) -> Result<String> {
    resolve_link(value, config, route)
}

/// Normalize a site-root page link to its final page URL.
///
/// This reuses the same prefix and slug rules as emitted HTML links,
/// but returns a page-only [`UrlPath`] without any fragment.
pub fn normalize_site_root_page_url(value: &str, config: &SiteConfig) -> UrlPath {
    let (path, _) = split_path_fragment(value);
    let path = path.trim_start_matches('/');
    UrlPath::from_page(&build_prefixed_url(path, config))
}

/// Resolve site-root-relative links (/about, /posts/hello)
fn resolve_site_root(value: &str, config: &SiteConfig) -> Result<String> {
    let paths = config.paths();

    // Asset links: just add prefix, no slugification
    if is_asset_link(value, config) {
        let path = value.trim_start_matches('/');
        return Ok(paths.url_for_rel_path(path));
    }

    // Split path and fragment
    let (_, fragment) = split_path_fragment(value);
    let mut url = normalize_site_root_page_url(value, config).to_string();

    // Append slugified fragment if present
    if !fragment.is_empty() {
        url.push('#');
        url.push_str(&slugify_fragment(fragment, &config.build.slug));
    }

    Ok(url)
}

/// Resolve file-relative links (./image.png, ../other)
///
/// For non-index files, relative paths are adjusted because
/// `foo.typ` becomes `foo/index.html` (one directory deeper)
fn resolve_file_relative(value: &str, route: &PageRoute) -> String {
    // External links (https://, mailto:) are already handled by LinkKind::External,
    // but bare domains without scheme (example.com) fall through here
    if value.contains("://") {
        return value.to_string();
    }

    // For index files, relative paths work as-is
    if route.is_index {
        return value.to_string();
    }

    // For non-index files: a.typ -> a/index.html (one level deeper)
    // All relative paths need ../ to compensate
    format!("../{value}")
}

// =============================================================================
// Path Prefix Handling
// =============================================================================

/// Build a URL with path_prefix, avoiding double-prefixing
fn build_prefixed_url(path: &str, config: &SiteConfig) -> String {
    let paths = config.paths();
    let slugified = slugify_path(path, &config.build.slug);
    let slugified_str = slugified.to_string_lossy();

    if has_path_prefix(path, config) {
        format!("/{slugified_str}")
    } else {
        paths.url_for_rel_path(&*slugified_str)
    }
}

/// Check if a path already contains the configured path_prefix
fn has_path_prefix(path: &str, config: &SiteConfig) -> bool {
    let paths = config.paths();

    if !paths.has_prefix() {
        return false;
    }

    let prefix = paths.prefix();
    let prefix_str = prefix.to_string_lossy();

    path_starts_with_segment(path, &prefix_str)
}

/// Check if path starts with a given segment (not just string prefix)
fn path_starts_with_segment(path: &str, segment: &str) -> bool {
    if path == segment {
        return true;
    }
    let with_slash = format!("{segment}/");
    path.starts_with(&with_slash)
}

/// Check if a path is an asset link
fn is_asset_link(path: &str, config: &SiteConfig) -> bool {
    let first_component = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();

    config
        .build
        .assets
        .nested
        .iter()
        .any(|entry| entry.output_name() == first_component)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::section::build::assets::NestedEntry;
    use crate::core::UrlPath;
    use std::path::PathBuf;

    fn test_route(is_index: bool) -> PageRoute {
        PageRoute {
            source: PathBuf::from("test.typ"),
            is_index,
            is_404: false,
            permalink: UrlPath::from_page("/test/"),
            output_file: PathBuf::from("public/test/index.html"),
            output_dir: PathBuf::from("public/test"),
            full_url: "https://example.com/test/".to_string(),
        }
    }

    fn assert_resolve_cases(route: &PageRoute, cases: &[(&str, &str)]) {
        let config = SiteConfig::default();
        for (input, expected) in cases {
            assert_eq!(
                resolve_link(input, &config, route).unwrap(),
                *expected,
                "{input:?}"
            );
        }
    }

    // =========================================================================
    // resolve_link Tests
    // =========================================================================

    #[test]
    fn test_resolve_index_cases() {
        let route = test_route(true);
        assert_resolve_cases(
            &route,
            &[
                ("https://example.com", "https://example.com"),
                ("mailto:user@example.com", "mailto:user@example.com"),
                ("#section", "#section"),
                ("#my-heading", "#my-heading"),
                ("./img.png", "./img.png"),
                ("../doc.pdf", "../doc.pdf"),
            ],
        );
        let config = SiteConfig::default();
        assert!(
            resolve_link("/about", &config, &route)
                .unwrap()
                .starts_with('/')
        );
    }

    #[test]
    fn test_resolve_non_index_cases() {
        let route = test_route(false);
        assert_resolve_cases(
            &route,
            &[
                ("./img.png", ".././img.png"),
                ("../doc.pdf", "../../doc.pdf"),
            ],
        );
    }

    #[test]
    fn test_resolve_empty_error() {
        let config = SiteConfig::default();
        let route = test_route(true);
        assert!(resolve_link("", &config, &route).is_err());
    }

    // =========================================================================
    // process_link_value Tests (URL output)
    // =========================================================================

    #[test]
    fn test_process_link_value_dispatch() {
        let config = SiteConfig::default();
        let route = test_route(true);

        let result = process_link_value("/about", &config, &route).unwrap();
        assert!(result.starts_with('/'));

        let result = process_link_value("#section", &config, &route).unwrap();
        assert!(result.starts_with('#'));

        let result = process_link_value("https://example.com", &config, &route).unwrap();
        assert_eq!(result, "https://example.com");
    }

    #[test]
    fn test_process_link_value_empty_error() {
        let config = SiteConfig::default();
        let route = test_route(true);
        assert!(process_link_value("", &config, &route).is_err());
    }

    #[test]
    fn transform_keeps_link_and_heading_payloads_in_sync_with_attrs() {
        use crate::compiler::family::TolaSite;
        use tola_vdom::core::ExtractFamily;
        use tola_vdom::families::{HeadingFamily, LinkFamily};

        let config = SiteConfig::default();
        let route = test_route(true);
        let root = TolaSite::element("main", Attrs::new())
            .child(TolaSite::element(
                "a",
                Attrs::from([("href", "#My Section")]),
            ))
            .child(TolaSite::element("h2", Attrs::from([("id", "My Section")])));
        let indexed = TolaSite::indexer().transform(Document::new(root));

        let transformed = LinkTransform::new(&config, &route).transform(indexed);

        let link = transformed.find(|elem| elem.is_tag("a")).unwrap();
        let link_data = ExtractFamily::<LinkFamily>::get(&link.ext).unwrap();
        assert_eq!(link.get_attr("href"), Some("#my-section"));
        assert_eq!(link_data.href.as_deref(), Some("#my-section"));

        let heading = transformed.find(|elem| elem.is_tag("h2")).unwrap();
        let heading_data = ExtractFamily::<HeadingFamily>::get(&heading.ext).unwrap();
        assert_eq!(heading.get_attr("id"), Some("my-section"));
        assert_eq!(heading_data.id.as_deref(), Some("my-section"));
    }

    // =========================================================================
    // Path Prefix Tests
    // =========================================================================

    #[test]
    fn test_path_starts_with_segment_cases() {
        for (path, prefix, expected) in [
            ("blog", "blog", true),
            ("docs", "docs", true),
            ("blog/post-1", "blog", true),
            ("blog/2024/post", "blog", true),
            ("blogger/post", "blog", false),
            ("blogging", "blog", false),
            ("about", "blog", false),
            ("posts/blog", "blog", false),
        ] {
            assert_eq!(path_starts_with_segment(path, prefix), expected, "{path:?}");
        }
    }

    // =========================================================================
    // Non-index File Relative Path Tests
    // =========================================================================

    fn test_route_non_index() -> PageRoute {
        PageRoute {
            source: PathBuf::from("content/posts/hello.typ"),
            is_index: false,
            is_404: false,
            permalink: UrlPath::from_page("/posts/hello/"),
            output_file: PathBuf::from("public/posts/hello/index.html"),
            output_dir: PathBuf::from("public/posts/hello"),
            full_url: "https://example.com/posts/hello/".to_string(),
        }
    }

    #[test]
    fn test_resolve_non_index_relative_paths() {
        let route = test_route_non_index();
        assert_resolve_cases(
            &route,
            &[
                ("./image.png", ".././image.png"),
                ("image.png", "../image.png"),
                ("hello/cat.svg", "../hello/cat.svg"),
                ("../doc.pdf", "../../doc.pdf"),
            ],
        );
    }

    #[test]
    fn test_is_asset_link_uses_current_config() {
        let mut first = SiteConfig::default();
        first.build.assets.nested = vec![NestedEntry::Simple("images".into())];

        let mut second = SiteConfig::default();
        second.build.assets.nested = vec![NestedEntry::Simple("media".into())];

        assert!(is_asset_link("/images/logo.png", &first));
        assert!(!is_asset_link("/media/logo.png", &first));

        assert!(is_asset_link("/media/logo.png", &second));
        assert!(!is_asset_link("/images/logo.png", &second));
    }
}
