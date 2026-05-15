//! URL conflict detection for pages, assets, and generated outputs.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use crate::asset::{scan_content_assets, scan_nested_assets};
use crate::config::SiteConfig;
use crate::config::section::build::AtomicCssConfig;
use crate::core::UrlPath;
use crate::log;
use crate::page::CompiledPage;
use crate::utils::plural_s;

/// URL ownership map: URL -> list of owners claiming that URL.
pub type UrlOwnerMap = FxHashMap<UrlPath, Vec<UrlOwner>>;

/// A resource that claims a public URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlOwner {
    /// A source file that will be written or routed at the URL.
    Source(PathBuf),
    /// A generated output configured in `tola.toml`.
    Generated(String),
}

impl UrlOwner {
    fn source(path: PathBuf) -> Self {
        Self::Source(path)
    }

    fn generated(label: impl Into<String>) -> Self {
        Self::Generated(label.into())
    }

    fn relativize(&self, root: &Path) -> Self {
        match self {
            Self::Source(path) => Self::Source(
                path.strip_prefix(root)
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|_| path.clone()),
            ),
            Self::Generated(label) => Self::Generated(label.clone()),
        }
    }
}

impl std::fmt::Display for UrlOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(path) => write!(f, "{}", path.display()),
            Self::Generated(label) => f.write_str(label),
        }
    }
}

/// A URL conflict: multiple resources claim the same URL
#[derive(Debug, Clone)]
pub struct UrlConflict {
    /// The conflicting URL
    pub url: UrlPath,
    /// All owners claiming this URL.
    pub owners: Vec<UrlOwner>,
}

/// Collect all URL -> owner mappings.
///
/// This is the first phase of conflict detection. It gathers all URLs
/// that will be used by pages, assets, and generated outputs, without checking
/// for conflicts yet.
pub fn collect_url_owners(pages: &[CompiledPage], config: &SiteConfig) -> UrlOwnerMap {
    let mut url_owners = UrlOwnerMap::default();

    collect_asset_urls(&mut url_owners, config);

    // Collect generated public outputs.
    collect_generated_outputs(&mut url_owners, config);

    // Collect pages (permalinks + aliases)
    for page in pages {
        collect_page_urls(&mut url_owners, page);
    }

    url_owners
}

fn collect_asset_urls(url_owners: &mut UrlOwnerMap, config: &SiteConfig) {
    for asset in scan_nested_assets(config) {
        url_owners
            .entry(asset.url)
            .or_default()
            .push(UrlOwner::source(asset.source));
    }

    for asset in crate::asset::scan_flatten_assets(config) {
        url_owners
            .entry(asset.url)
            .or_default()
            .push(UrlOwner::source(asset.source));
    }

    for asset in scan_content_assets(config) {
        url_owners
            .entry(asset.url)
            .or_default()
            .push(UrlOwner::source(asset.source));
    }
}

fn collect_generated_outputs(url_owners: &mut UrlOwnerMap, config: &SiteConfig) {
    if config.build.atomic_css.enable {
        let route = AtomicCssConfig::output_route();
        url_owners
            .entry(route.clone())
            .or_default()
            .push(UrlOwner::generated(format!("build.atomic_css ({route})")));
    }

    for (idx, feed) in config.site.seo.feed_outputs().iter().enumerate() {
        url_owners
            .entry(feed.url.url_path())
            .or_default()
            .push(UrlOwner::generated(format!(
                "site.seo.feeds[{idx}] ({} -> {})",
                feed.format.as_str(),
                feed.url
            )));
    }

    if config.site.seo.sitemap.enable {
        url_owners
            .entry(config.site.seo.sitemap.url.url_path())
            .or_default()
            .push(UrlOwner::generated(format!(
                "site.seo.sitemap ({})",
                config.site.seo.sitemap.url
            )));
    }
}

/// Collect all URLs from a single page: permalink and aliases
fn collect_page_urls(url_owners: &mut UrlOwnerMap, page: &CompiledPage) {
    // Skip 404 page (it's a fallback file, not a route target)
    if page.route.is_404 {
        return;
    }

    let source = &page.route.source;

    // Page permalink
    url_owners
        .entry(page.route.permalink.clone())
        .or_default()
        .push(UrlOwner::source(source.clone()));

    // Page aliases (redirect URLs pointing to this page)
    if let Some(meta) = &page.content_meta {
        for alias in &meta.aliases {
            let alias_url = UrlPath::from_page(alias);
            url_owners
                .entry(alias_url)
                .or_default()
                .push(UrlOwner::source(source.clone()));
        }
    }
}

/// Detect URL conflicts (URLs claimed by multiple resources)
///
/// This is the second phase of conflict detection. It finds all URLs
/// that have more than one owner, which indicates a conflict.
///
/// Source paths are converted to relative paths using the provided root.
pub fn detect_conflicts(url_owners: &UrlOwnerMap, root: &Path) -> Vec<UrlConflict> {
    url_owners
        .iter()
        .filter(|(_, sources)| sources.len() > 1)
        .map(|(url, sources)| UrlConflict {
            url: url.clone(),
            owners: relativize_owners(sources, root),
        })
        .collect()
}

/// Convert absolute source paths to relative paths.
fn relativize_owners(owners: &[UrlOwner], root: &Path) -> Vec<UrlOwner> {
    owners.iter().map(|owner| owner.relativize(root)).collect()
}

/// Print conflicts using the standard log format
///
/// Output format:
/// ```text
/// [error] url conflicts (2 urls)
/// [url] /foo/ (3 owners)
///   - content/a.typ
///   - content/b.typ
/// ```
pub fn print_conflicts(conflicts: &[UrlConflict]) {
    if conflicts.is_empty() {
        return;
    }

    let total_owners: usize = conflicts.iter().map(|c| c.owners.len()).sum();
    log!("error"; "url conflicts ({} url{}, {} owner{})",
        conflicts.len(), plural_s(conflicts.len()),
        total_owners, plural_s(total_owners));

    for conflict in conflicts {
        eprintln!();
        log!(
            "url";
            "{} ({} owner{})",
            conflict.url,
            conflict.owners.len(),
            plural_s(conflict.owners.len())
        );
        for source in &conflict.owners {
            eprintln!("  - {}", source);
        }
    }
}

/// Format conflicts as a string (for error messages)
pub fn format_conflicts(conflicts: &[UrlConflict]) -> String {
    conflicts
        .iter()
        .map(format_single_conflict)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a single conflict for display
fn format_single_conflict(conflict: &UrlConflict) -> String {
    let mut lines = vec![format!("{} ({})", conflict.url, conflict.owners.len())];
    for source in &conflict.owners {
        lines.push(format!("  - {source}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FeedFormat;
    use crate::config::section::build::assets::{FlattenEntry, NestedEntry};
    use crate::config::section::site::FeedConfig;
    use crate::page::PageRoute;
    use tempfile::TempDir;

    fn make_page(source: &str, permalink: &str) -> CompiledPage {
        CompiledPage {
            route: PageRoute {
                source: PathBuf::from(source),
                is_index: false,
                is_404: false,
                permalink: UrlPath::from_page(permalink),
                output_file: PathBuf::from("public/test/index.html"),
                output_dir: PathBuf::from("public/test"),
                full_url: format!("https://example.com{}", permalink),
            },
            lastmod: None,
            content_meta: None,
            compiled_html: None,
            feed_body: None,
        }
    }

    fn make_url_owners(pages: &[CompiledPage]) -> UrlOwnerMap {
        let mut url_owners = UrlOwnerMap::default();
        for page in pages {
            url_owners
                .entry(page.route.permalink.clone())
                .or_default()
                .push(UrlOwner::source(page.route.source.clone()));
        }
        url_owners
    }

    #[test]
    fn test_no_conflicts() {
        let pages = vec![
            make_page("content/a.typ", "/a/"),
            make_page("content/b.typ", "/b/"),
            make_page("content/c.typ", "/c/"),
        ];

        let url_owners = make_url_owners(&pages);
        let conflicts = detect_conflicts(&url_owners, Path::new(""));
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_page_vs_page_conflict() {
        let pages = vec![
            make_page("content/a.typ", "/foo/"),
            make_page("content/b.typ", "/foo/"),
        ];

        let url_owners = make_url_owners(&pages);
        let conflicts = detect_conflicts(&url_owners, Path::new(""));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_page("/foo/"));
        assert_eq!(conflicts[0].owners.len(), 2);
    }

    #[test]
    fn test_three_way_conflict() {
        let pages = vec![
            make_page("content/a.typ", "/foo/"),
            make_page("content/b.typ", "/foo/"),
            make_page("content/c.typ", "/foo/"),
        ];

        let url_owners = make_url_owners(&pages);
        let conflicts = detect_conflicts(&url_owners, Path::new(""));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_page("/foo/"));
        assert_eq!(conflicts[0].owners.len(), 3);
    }

    #[test]
    fn test_multiple_conflicts() {
        let mut url_owners = UrlOwnerMap::default();

        // Conflict 1: /foo/
        url_owners
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(UrlOwner::source(PathBuf::from("content/a.typ")));
        url_owners
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(UrlOwner::source(PathBuf::from("content/b.typ")));

        // Conflict 2: /bar/
        url_owners
            .entry(UrlPath::from_page("/bar/"))
            .or_default()
            .push(UrlOwner::source(PathBuf::from("content/c.typ")));
        url_owners
            .entry(UrlPath::from_page("/bar/"))
            .or_default()
            .push(UrlOwner::source(PathBuf::from("assets/bar")));

        // No conflict: /baz/
        url_owners
            .entry(UrlPath::from_page("/baz/"))
            .or_default()
            .push(UrlOwner::source(PathBuf::from("content/d.typ")));

        let conflicts = detect_conflicts(&url_owners, Path::new(""));
        assert_eq!(conflicts.len(), 2);
    }

    #[test]
    fn test_relative_paths() {
        let mut url_owners = UrlOwnerMap::default();
        url_owners
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(UrlOwner::source(PathBuf::from("/project/content/a.typ")));
        url_owners
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(UrlOwner::source(PathBuf::from("/project/content/b.typ")));

        let conflicts = detect_conflicts(&url_owners, Path::new("/project"));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].owners[0],
            UrlOwner::source(PathBuf::from("content/a.typ"))
        );
        assert_eq!(
            conflicts[0].owners[1],
            UrlOwner::source(PathBuf::from("content/b.typ"))
        );
    }

    #[test]
    fn test_format_conflicts() {
        let conflicts = vec![UrlConflict {
            url: UrlPath::from_page("/foo/"),
            owners: vec![
                UrlOwner::source(PathBuf::from("content/a.typ")),
                UrlOwner::source(PathBuf::from("content/b.typ")),
            ],
        }];

        let formatted = format_conflicts(&conflicts);
        assert!(formatted.contains("/foo/"));
        assert!(formatted.contains("content/a.typ"));
        assert!(formatted.contains("content/b.typ"));
    }

    #[test]
    fn feed_url_conflicts_with_file_asset_url() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("feed.xml");
        std::fs::write(&source, "asset feed").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.flatten = vec![FlattenEntry::new(source, "/feed.xml")];
        config.site.seo.feeds = vec![FeedConfig {
            format: FeedFormat::Rss,
            url: "/feed.xml".into(),
            features: Vec::new(),
        }];

        let url_owners = collect_url_owners(&[], &config);
        let conflicts = detect_conflicts(&url_owners, config.get_root());

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_asset("/feed.xml"));
        assert_eq!(conflicts[0].owners.len(), 2);
        assert!(
            conflicts[0]
                .owners
                .iter()
                .any(|source| source.to_string().contains("site.seo.feeds[0]"))
        );
    }

    #[test]
    fn feed_url_conflicts_with_dir_asset_url() {
        let dir = TempDir::new().unwrap();
        let assets_dir = dir.path().join("assets");
        std::fs::create_dir_all(&assets_dir).unwrap();
        std::fs::write(assets_dir.join("feed.xml"), "asset feed").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.nested = vec![NestedEntry::new(assets_dir, "/assets")];
        config.site.seo.feeds = vec![FeedConfig {
            format: FeedFormat::Rss,
            url: "/assets/feed.xml".into(),
            features: Vec::new(),
        }];

        let url_owners = collect_url_owners(&[], &config);
        let conflicts = detect_conflicts(&url_owners, config.get_root());

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_asset("/assets/feed.xml"));
        assert_eq!(conflicts[0].owners.len(), 2);
    }

    #[test]
    fn sitemap_url_conflicts_with_file_asset_url() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("sitemap.xml");
        std::fs::write(&source, "asset sitemap").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.flatten = vec![FlattenEntry::new(source, "/sitemap.xml")];
        config.site.seo.sitemap.enable = true;
        config.site.seo.sitemap.url = "/sitemap.xml".into();

        let url_owners = collect_url_owners(&[], &config);
        let conflicts = detect_conflicts(&url_owners, config.get_root());

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_asset("/sitemap.xml"));
        assert_eq!(conflicts[0].owners.len(), 2);
        assert!(
            conflicts[0]
                .owners
                .iter()
                .any(|owner| owner.to_string().contains("site.seo.sitemap"))
        );
    }

    #[test]
    fn generated_urls_conflict_with_each_other() {
        let dir = TempDir::new().unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.atomic_css.enable = true;
        config.site.seo.sitemap.enable = true;
        config.site.seo.sitemap.url = "/.tola/atomic.css".into();

        let url_owners = collect_url_owners(&[], &config);
        let conflicts = detect_conflicts(&url_owners, config.get_root());

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_asset("/.tola/atomic.css"));
        assert!(
            conflicts[0]
                .owners
                .iter()
                .any(|owner| owner.to_string().contains("build.atomic_css"))
        );
        assert!(
            conflicts[0]
                .owners
                .iter()
                .any(|owner| owner.to_string().contains("site.seo.sitemap"))
        );
    }
}
