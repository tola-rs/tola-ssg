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

use std::path::Path;

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
    let slugified = slugify_path(path, &config.build.slug);
    UrlPath::from_page(&slugified.to_string_lossy())
}

/// Resolve site-root-relative links (/about, /posts/hello)
fn resolve_site_root(value: &str, config: &SiteConfig) -> Result<String> {
    if let Some(href) = crate::asset::resolve_asset_href(value, config) {
        return Ok(href);
    }
    if is_public_file_url(value) {
        return Ok(resolve_public_file(value, config));
    }

    // Split path and fragment
    let (_, fragment) = split_path_fragment(value);
    let site_path = normalize_site_root_page_url(value, config);
    let mut url = config.paths().url_for_site_path(site_path.as_str());

    // Append slugified fragment if present
    if !fragment.is_empty() {
        url.push('#');
        url.push_str(&slugify_fragment(fragment, &config.build.slug));
    }

    Ok(url)
}

fn is_public_file_url(value: &str) -> bool {
    let (path, _) = split_path_fragment(value);
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some()
}

fn resolve_public_file(value: &str, config: &SiteConfig) -> String {
    let idx = value.find(['?', '#']).unwrap_or(value.len());
    let path = value[..idx].trim_start_matches('/');
    let suffix = &value[idx..];
    format!("{}{}", config.paths().url_for_site_path(path), suffix)
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

/// Check if a path is an asset link
fn is_asset_link(path: &str, config: &SiteConfig) -> bool {
    crate::asset::is_asset_url(path, config)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::section::build::assets::{FlattenEntry, NestedEntry};
    use crate::core::UrlPath;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

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
    fn site_root_normalization_uses_site_path_not_public_prefix() {
        let mut config = SiteConfig::default();
        config.build.path_prefix = PathBuf::from("docs/blog");

        assert_eq!(
            normalize_site_root_page_url("/posts/Hello World", &config),
            UrlPath::from_page("/posts/hello-world/")
        );

        let route = test_route(true);
        assert_eq!(
            resolve_link("/posts/Hello World", &config, &route).unwrap(),
            "/docs/blog/posts/hello-world/"
        );
        assert_eq!(
            resolve_link("/docs/blog", &config, &route).unwrap(),
            "/docs/blog/docs/blog/"
        );
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
        first.build.assets.nested = vec![NestedEntry::new("images", "/images")];

        let mut second = SiteConfig::default();
        second.build.assets.nested = vec![NestedEntry::new("media", "/media")];

        assert!(is_asset_link("/images/logo.png", &first));
        assert!(!is_asset_link("/media/logo.png", &first));

        assert!(is_asset_link("/media/logo.png", &second));
        assert!(!is_asset_link("/images/logo.png", &second));
    }

    #[test]
    fn generated_asset_links_are_not_page_links() {
        let config = SiteConfig::default();
        let route = test_route(true);

        assert_eq!(
            resolve_link("/.tola/enhance.css", &config, &route).unwrap(),
            "/.tola/enhance.css"
        );
    }

    #[test]
    fn prefixed_generated_asset_links_are_preserved() {
        let mut config = SiteConfig::default();
        config.build.path_prefix = PathBuf::from("docs/blog");
        let route = test_route(true);

        assert_eq!(
            resolve_link("/docs/blog/.tola/enhance.css", &config, &route).unwrap(),
            "/docs/blog/.tola/enhance.css"
        );
    }

    #[test]
    fn file_asset_links_keep_query_and_fragment() {
        let mut config = SiteConfig::default();
        config.build.path_prefix = PathBuf::from("docs/blog");
        config.build.assets.flatten = vec![FlattenEntry::new("favicon.ico", "/favicon.ico")];
        let route = test_route(true);

        assert_eq!(
            resolve_link("/favicon.ico?v=1#icon", &config, &route).unwrap(),
            "/docs/blog/favicon.ico?v=1#icon"
        );
    }

    #[test]
    fn colocated_asset_links_use_asset_url_rules() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(content_dir.join("posts/hello")).unwrap();
        fs::write(content_dir.join("posts/hello/image.png"), "image").unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.path_prefix = PathBuf::from("docs/blog");
        config.build.assets.colocated = true;
        let route = test_route(true);

        assert_eq!(
            resolve_link("/posts/hello/image.png", &config, &route).unwrap(),
            "/docs/blog/posts/hello/image.png"
        );
    }

    #[test]
    fn missing_colocated_asset_with_extension_is_not_pageified() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.path_prefix = PathBuf::from("docs/blog");
        config.build.assets.colocated = true;
        let route = test_route(true);

        assert_eq!(
            resolve_link("/posts/hello/missing.png", &config, &route).unwrap(),
            "/docs/blog/posts/hello/missing.png"
        );
    }
}
