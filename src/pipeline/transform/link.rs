//! Link and URL processor (Indexed → Indexed).
//!
//! Processes all URL-related attributes:
//! - Link family: href/src attributes (absolute, relative, fragment, external)
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

use rustc_hash::FxHashSet;
use std::ffi::OsString;
use std::sync::OnceLock;

use anyhow::Result;
use tola_vdom::prelude::*;

use crate::compiler::family::{Indexed, TolaSite::FamilyKind};
use crate::compiler::page::PageRoute;
use crate::config::SiteConfig;
use crate::core::LinkKind;
use crate::utils::path::route::split_path_fragment;
use crate::utils::path::slug::{slugify_fragment, slugify_path};

// =============================================================================
// VDOM Transform
// =============================================================================

/// Processes link href/src and heading id attributes in Indexed VDOM.
pub struct LinkTransform<'a> {
    config: &'a SiteConfig,
    route: &'a PageRoute,
}

impl<'a> LinkTransform<'a> {
    pub fn new(config: &'a SiteConfig, route: &'a PageRoute) -> Self {
        Self { config, route }
    }

    /// Process a URL attribute (href or src).
    fn process_url(&self, elem: &mut Element<Indexed>, attr: &str) {
        let Some(val) = elem.get_attr(attr).map(|s| s.to_string()) else {
            return;
        };

        // Process the link
        if let Ok(processed) = process_link_value(&val, self.config, self.route) {
            elem.set_attr(attr, processed);
        }
    }
}

impl Transform<Indexed> for LinkTransform<'_> {
    type To = Indexed;

    fn transform(self, mut doc: Document<Indexed>) -> Document<Indexed> {
        // Link href/src
        doc.modify_by::<FamilyKind::Link, _>(|elem| {
            self.process_url(elem, "href");
            self.process_url(elem, "src");
        });

        // Heading id slugify
        let slug_config = &self.config.build.slug;
        doc.modify_by::<FamilyKind::Heading, _>(|elem| {
            if let Some(id) = elem.get_attr("id").map(|s| s.to_string()) {
                elem.set_attr("id", slugify_fragment(&id, slug_config));
            }
        });

        doc
    }
}

// =============================================================================
// Link Processing Logic
// =============================================================================

/// Resolve a link to its final URL string.
///
/// This is the main entry point for link resolution. It classifies the link
/// syntactically using [`LinkKind`], then resolves it based on context.
///
/// # Link Types
///
/// - External URLs (https://, mailto:, etc.) → preserved as-is
/// - Fragment anchors (#section) → slugified
/// - Site-root links (/about) → prefixed and slugified
/// - File-relative (./image.png) → adjusted for output structure
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

/// Process a link value (href or src attribute).
///
/// Alias for [`resolve_link`] for clarity at call sites.
#[inline]
pub fn process_link_value(value: &str, config: &SiteConfig, route: &PageRoute) -> Result<String> {
    resolve_link(value, config, route)
}

/// Resolve site-root-relative links (/about, /posts/hello).
fn resolve_site_root(value: &str, config: &SiteConfig) -> Result<String> {
    let paths = config.paths();

    // Asset links: just add prefix, no slugification
    if is_asset_link(value, config) {
        let path = value.trim_start_matches('/');
        return Ok(paths.url_for_rel_path(path));
    }

    // Split path and fragment
    let (path, fragment) = split_path_fragment(value);
    let path = path.trim_start_matches('/');

    // Build URL with proper prefix handling
    let mut url = build_prefixed_url(path, config);

    // Append slugified fragment if present
    if !fragment.is_empty() {
        url.push('#');
        url.push_str(&slugify_fragment(fragment, &config.build.slug));
    }

    Ok(url)
}

/// Resolve file-relative links (./image.png, ../other).
///
/// For non-index files, relative paths are adjusted because
/// `foo.typ` becomes `foo/index.html` (one directory deeper).
fn resolve_file_relative(value: &str, route: &PageRoute) -> String {
    // External links (https://, mailto:) are already handled by LinkKind::External,
    // but bare domains without scheme (example.com) fall through here
    if value.contains("://") {
        return value.to_string();
    }

    // For index files, relative paths work as-is (assets in same directory)
    if route.is_index {
        return value.to_string();
    }

    // For non-index files with colocated assets directory:
    // ./image.png and image.png stay as-is (assets are copied to output_dir)
    // Only ../ paths need adjustment (they reference parent directory)
    if route.colocated_dir.is_some() && !value.starts_with("../") {
        return value.to_string();
    }

    // For non-index files without colocated assets:
    // ./image.png → ../image.png (go up one level because foo.typ → foo/index.html)
    format!("../{value}")
}

// =============================================================================
// Path Prefix Handling
// =============================================================================

/// Build a URL with path_prefix, avoiding double-prefixing.
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

/// Check if a path already contains the configured path_prefix.
fn has_path_prefix(path: &str, config: &SiteConfig) -> bool {
    let paths = config.paths();

    if !paths.has_prefix() {
        return false;
    }

    let prefix = paths.prefix();
    let prefix_str = prefix.to_string_lossy();

    path_starts_with_segment(path, &prefix_str)
}

/// Check if path starts with a given segment (not just string prefix).
fn path_starts_with_segment(path: &str, segment: &str) -> bool {
    if path == segment {
        return true;
    }
    let with_slash = format!("{segment}/");
    path.starts_with(&with_slash)
}

// =============================================================================
// Asset Link Detection
// =============================================================================

static ASSET_TOP_LEVELS: OnceLock<FxHashSet<OsString>> = OnceLock::new();

/// Check if a path is an asset link.
fn is_asset_link(path: &str, config: &SiteConfig) -> bool {
    let asset_top_levels = get_asset_top_levels(config);

    let first_component = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();

    asset_top_levels.contains(first_component.as_ref() as &std::ffi::OsStr)
}

/// Get top-level asset directory names from all nested directories.
fn get_asset_top_levels(config: &SiteConfig) -> &'static FxHashSet<OsString> {
    ASSET_TOP_LEVELS.get_or_init(|| {
        let mut set = FxHashSet::default();
        for entry in &config.build.assets.nested {
            // Add the output name (what appears in URLs)
            set.insert(OsString::from(entry.output_name()));
        }
        set
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::core::UrlPath;

    fn test_route(is_index: bool) -> PageRoute {
        PageRoute {
            source: PathBuf::from("test.typ"),
            is_index,
            is_404: false,
            colocated_dir: None,
            permalink: UrlPath::from_page("/test/"),
            output_file: PathBuf::from("public/test/index.html"),
            output_dir: PathBuf::from("public/test"),
            full_url: "https://example.com/test/".to_string(),
            relative: "test".to_string(),
        }
    }

    // =========================================================================
    // resolve_link Tests
    // =========================================================================

    #[test]
    fn test_resolve_external() {
        let config = SiteConfig::default();
        let route = test_route(true);

        assert_eq!(
            resolve_link("https://example.com", &config, &route).unwrap(),
            "https://example.com"
        );
        assert_eq!(
            resolve_link("mailto:user@example.com", &config, &route).unwrap(),
            "mailto:user@example.com"
        );
    }

    #[test]
    fn test_resolve_fragment() {
        let config = SiteConfig::default();
        let route = test_route(true);

        assert_eq!(
            resolve_link("#section", &config, &route).unwrap(),
            "#section"
        );
        assert_eq!(
            resolve_link("#my-heading", &config, &route).unwrap(),
            "#my-heading"
        );
    }

    #[test]
    fn test_resolve_site_root() {
        let config = SiteConfig::default();
        let route = test_route(true);

        let url = resolve_link("/about", &config, &route).unwrap();
        assert!(url.starts_with('/'));
    }

    #[test]
    fn test_resolve_file_relative_index() {
        let route = test_route(true);
        let config = SiteConfig::default();

        // Index files: relative paths work as-is
        assert_eq!(
            resolve_link("./img.png", &config, &route).unwrap(),
            "./img.png"
        );
        assert_eq!(
            resolve_link("../doc.pdf", &config, &route).unwrap(),
            "../doc.pdf"
        );
    }

    #[test]
    fn test_resolve_file_relative_non_index() {
        let route = test_route(false);
        let config = SiteConfig::default();

        // Non-index without colocated: paths get ../ prefix
        assert_eq!(
            resolve_link("./img.png", &config, &route).unwrap(),
            ".././img.png"
        );
        assert_eq!(
            resolve_link("../doc.pdf", &config, &route).unwrap(),
            "../../doc.pdf"
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

    // =========================================================================
    // Path Prefix Tests
    // =========================================================================

    #[test]
    fn test_path_starts_with_segment_exact_match() {
        assert!(path_starts_with_segment("blog", "blog"));
        assert!(path_starts_with_segment("docs", "docs"));
    }

    #[test]
    fn test_path_starts_with_segment_with_subpath() {
        assert!(path_starts_with_segment("blog/post-1", "blog"));
        assert!(path_starts_with_segment("blog/2024/post", "blog"));
    }

    #[test]
    fn test_path_starts_with_segment_partial_no_match() {
        assert!(!path_starts_with_segment("blogger/post", "blog"));
        assert!(!path_starts_with_segment("blogging", "blog"));
    }

    #[test]
    fn test_path_starts_with_segment_different_prefix() {
        assert!(!path_starts_with_segment("about", "blog"));
        assert!(!path_starts_with_segment("posts/blog", "blog"));
    }

    // =========================================================================
    // Colocated Assets Tests
    // =========================================================================

    fn test_route_colocated() -> PageRoute {
        PageRoute {
            source: PathBuf::from("content/posts/hello.typ"),
            is_index: false,
            is_404: false,
            colocated_dir: Some(PathBuf::from("content/posts/hello")),
            permalink: UrlPath::from_page("/posts/hello/"),
            output_file: PathBuf::from("public/posts/hello/index.html"),
            output_dir: PathBuf::from("public/posts/hello"),
            full_url: "https://example.com/posts/hello/".to_string(),
            relative: "posts/hello".to_string(),
        }
    }

    #[test]
    fn test_resolve_colocated_asset() {
        let route = test_route_colocated();
        let config = SiteConfig::default();

        // With colocated_dir set, ./image.png is preserved (assets copied by asset.rs)
        assert_eq!(
            resolve_link("./image.png", &config, &route).unwrap(),
            "./image.png"
        );
    }

    #[test]
    fn test_resolve_colocated_nested_asset() {
        let route = test_route_colocated();
        let config = SiteConfig::default();

        assert_eq!(
            resolve_link("./assets/logo.svg", &config, &route).unwrap(),
            "./assets/logo.svg"
        );
    }

    #[test]
    fn test_resolve_colocated_parent_path_adjusted() {
        let route = test_route_colocated();
        let config = SiteConfig::default();

        // ../other paths are NOT colocated, get adjusted
        assert_eq!(
            resolve_link("../doc.pdf", &config, &route).unwrap(),
            "../../doc.pdf"
        );
    }

    #[test]
    fn test_resolve_colocated_bare_path_preserved() {
        let route = test_route_colocated();
        let config = SiteConfig::default();

        // Bare paths (no ./ prefix) are also colocated assets
        assert_eq!(
            resolve_link("image.png", &config, &route).unwrap(),
            "image.png"
        );
    }

    #[test]
    fn test_resolve_non_index_without_colocated_dir() {
        let route = test_route(false);
        let config = SiteConfig::default();

        // Non-index without colocated_dir: all paths get ../ prefix
        assert_eq!(
            resolve_link("./img.png", &config, &route).unwrap(),
            ".././img.png"
        );
        assert_eq!(
            resolve_link("img.png", &config, &route).unwrap(),
            "../img.png"
        );
    }
}
