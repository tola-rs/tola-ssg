//! URL conflict detection for pages and assets.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use crate::asset::{scan_colocated_assets, scan_global_assets};
use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::log;
use crate::page::CompiledPage;
use crate::utils::plural_s;

/// URL sources map: URL -> list of source files claiming that URL.
pub type UrlSourceMap = FxHashMap<UrlPath, Vec<PathBuf>>;

/// A URL conflict: multiple resources claim the same URL.
#[derive(Debug, Clone)]
pub struct UrlConflict {
    /// The conflicting URL
    pub url: UrlPath,
    /// All source paths claiming this URL (relative to root)
    pub sources: Vec<PathBuf>,
}

/// Collect all URL -> sources mappings from pages and assets.
///
/// This is the first phase of conflict detection. It gathers all URLs
/// that will be used by pages and assets, without checking for conflicts yet.
pub fn collect_url_sources(pages: &[CompiledPage], config: &SiteConfig) -> UrlSourceMap {
    let mut url_sources = UrlSourceMap::default();

    // Collect global assets
    collect_global_assets(&mut url_sources, config);

    // Collect pages (permalinks + aliases + colocated assets)
    for page in pages {
        collect_page_urls(&mut url_sources, page);
    }

    url_sources
}

/// Collect global asset URLs into the map.
fn collect_global_assets(url_sources: &mut UrlSourceMap, config: &SiteConfig) {
    for asset in scan_global_assets(config) {
        url_sources.entry(asset.url).or_default().push(asset.source);
    }

    // Also collect flatten assets
    for asset in crate::asset::scan_flatten_assets(config) {
        url_sources.entry(asset.url).or_default().push(asset.source);
    }
}

/// Collect all URLs from a single page: permalink, aliases, and colocated assets.
fn collect_page_urls(url_sources: &mut UrlSourceMap, page: &CompiledPage) {
    // Skip 404 page (it's a fallback file, not a route target)
    if page.route.is_404 {
        return;
    }

    let source = &page.route.source;

    // Page permalink
    url_sources
        .entry(page.route.permalink.clone())
        .or_default()
        .push(source.clone());

    // Page aliases (redirect URLs pointing to this page)
    if let Some(meta) = &page.content_meta {
        for alias in &meta.aliases {
            let alias_url = UrlPath::from_page(alias);
            url_sources
                .entry(alias_url)
                .or_default()
                .push(source.clone());
        }
    }

    // Colocated assets
    if let Some(dir) = &page.route.colocated_dir {
        for asset in scan_colocated_assets(dir, &page.route) {
            url_sources.entry(asset.url).or_default().push(asset.source);
        }
    }
}

/// Detect URL conflicts (URLs claimed by multiple resources).
///
/// This is the second phase of conflict detection. It finds all URLs
/// that have more than one source, which indicates a conflict.
///
/// Paths are converted to relative paths using the provided root.
pub fn detect_conflicts(url_sources: &UrlSourceMap, root: &Path) -> Vec<UrlConflict> {
    url_sources
        .iter()
        .filter(|(_, sources)| sources.len() > 1)
        .map(|(url, sources)| UrlConflict {
            url: url.clone(),
            sources: relativize_paths(sources, root),
        })
        .collect()
}

/// Convert absolute paths to relative paths.
fn relativize_paths(paths: &[PathBuf], root: &Path) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|p| p.strip_prefix(root).unwrap_or(p).to_path_buf())
        .collect()
}

/// Print conflicts using the standard log format.
///
/// Output format:
/// ```text
/// [error] permalink conflicts (2 urls)
/// [url] /foo/ (3 sources)
///   - content/a.typ
///   - content/b.typ
/// ```
pub fn print_conflicts(conflicts: &[UrlConflict]) {
    if conflicts.is_empty() {
        return;
    }

    let total_sources: usize = conflicts.iter().map(|c| c.sources.len()).sum();
    log!("error"; "permalink conflicts ({} url{}, {} source{})",
        conflicts.len(), plural_s(conflicts.len()),
        total_sources, plural_s(total_sources));

    for conflict in conflicts {
        eprintln!();
        log!("url"; "{} ({} source{})", conflict.url, conflict.sources.len(), plural_s(conflict.sources.len()));
        for source in &conflict.sources {
            eprintln!("  - {}", source.display());
        }
    }
}

/// Format conflicts as a string (for error messages).
pub fn format_conflicts(conflicts: &[UrlConflict]) -> String {
    conflicts
        .iter()
        .map(format_single_conflict)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a single conflict for display.
fn format_single_conflict(conflict: &UrlConflict) -> String {
    let mut lines = vec![format!("{} ({})", conflict.url, conflict.sources.len())];
    for source in &conflict.sources {
        lines.push(format!("  - {}", source.display()));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageRoute;

    fn make_page(source: &str, permalink: &str) -> CompiledPage {
        CompiledPage {
            route: PageRoute {
                source: PathBuf::from(source),
                is_index: false,
                is_404: false,
                colocated_dir: None,
                permalink: UrlPath::from_page(permalink),
                output_file: PathBuf::from("public/test/index.html"),
                output_dir: PathBuf::from("public/test"),
                full_url: format!("https://example.com{}", permalink),
                relative: "test".to_string(),
            },
            lastmod: None,
            content_meta: None,
            compiled_html: None,
        }
    }

    fn make_url_sources(pages: &[CompiledPage]) -> UrlSourceMap {
        let mut url_sources = UrlSourceMap::default();
        for page in pages {
            url_sources
                .entry(page.route.permalink.clone())
                .or_default()
                .push(page.route.source.clone());
        }
        url_sources
    }

    #[test]
    fn test_no_conflicts() {
        let pages = vec![
            make_page("content/a.typ", "/a/"),
            make_page("content/b.typ", "/b/"),
            make_page("content/c.typ", "/c/"),
        ];

        let url_sources = make_url_sources(&pages);
        let conflicts = detect_conflicts(&url_sources, Path::new(""));
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_page_vs_page_conflict() {
        let pages = vec![
            make_page("content/a.typ", "/foo/"),
            make_page("content/b.typ", "/foo/"),
        ];

        let url_sources = make_url_sources(&pages);
        let conflicts = detect_conflicts(&url_sources, Path::new(""));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_page("/foo/"));
        assert_eq!(conflicts[0].sources.len(), 2);
    }

    #[test]
    fn test_three_way_conflict() {
        let pages = vec![
            make_page("content/a.typ", "/foo/"),
            make_page("content/b.typ", "/foo/"),
            make_page("content/c.typ", "/foo/"),
        ];

        let url_sources = make_url_sources(&pages);
        let conflicts = detect_conflicts(&url_sources, Path::new(""));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].url, UrlPath::from_page("/foo/"));
        assert_eq!(conflicts[0].sources.len(), 3);
    }

    #[test]
    fn test_multiple_conflicts() {
        let mut url_sources = UrlSourceMap::default();

        // Conflict 1: /foo/
        url_sources
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(PathBuf::from("content/a.typ"));
        url_sources
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(PathBuf::from("content/b.typ"));

        // Conflict 2: /bar/
        url_sources
            .entry(UrlPath::from_page("/bar/"))
            .or_default()
            .push(PathBuf::from("content/c.typ"));
        url_sources
            .entry(UrlPath::from_page("/bar/"))
            .or_default()
            .push(PathBuf::from("assets/bar"));

        // No conflict: /baz/
        url_sources
            .entry(UrlPath::from_page("/baz/"))
            .or_default()
            .push(PathBuf::from("content/d.typ"));

        let conflicts = detect_conflicts(&url_sources, Path::new(""));
        assert_eq!(conflicts.len(), 2);
    }

    #[test]
    fn test_relative_paths() {
        let mut url_sources = UrlSourceMap::default();
        url_sources
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(PathBuf::from("/project/content/a.typ"));
        url_sources
            .entry(UrlPath::from_page("/foo/"))
            .or_default()
            .push(PathBuf::from("/project/content/b.typ"));

        let conflicts = detect_conflicts(&url_sources, Path::new("/project"));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].sources[0], PathBuf::from("content/a.typ"));
        assert_eq!(conflicts[0].sources[1], PathBuf::from("content/b.typ"));
    }

    #[test]
    fn test_format_conflicts() {
        let conflicts = vec![UrlConflict {
            url: UrlPath::from_page("/foo/"),
            sources: vec![
                PathBuf::from("content/a.typ"),
                PathBuf::from("content/b.typ"),
            ],
        }];

        let formatted = format_conflicts(&conflicts);
        assert!(formatted.contains("/foo/"));
        assert!(formatted.contains("content/a.typ"));
        assert!(formatted.contains("content/b.typ"));
    }
}
