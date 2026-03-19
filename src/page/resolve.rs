//! Page-link resolution helpers for backlink graph construction.

use std::path::Path;

use crate::compiler::page::ScannedPageLink;
use crate::config::SiteConfig;
use crate::core::{LinkKind, UrlPath};
use crate::page::StoredPageMap;
use crate::utils::path::route::{resolve_relative_url, split_path_fragment};

fn contains_page(store: &StoredPageMap, permalink: &UrlPath) -> bool {
    store
        .get_pages_with_drafts()
        .iter()
        .any(|page| &page.permalink == permalink)
}

fn permalink_for_source_candidate(store: &StoredPageMap, source: &Path) -> Option<UrlPath> {
    let candidates = [
        source.to_path_buf(),
        source.with_extension("typ"),
        source.with_extension("md"),
        source.join("index.typ"),
        source.join("index.md"),
    ];

    candidates
        .iter()
        .find_map(|candidate| store.get_permalink_by_source(candidate))
}

/// Resolve a scanned page-link candidate to a target page permalink.
///
/// This reuses existing link classification and relative-path resolution.
pub fn resolve_page_link_target(
    store: &StoredPageMap,
    current_permalink: &UrlPath,
    source_path: &Path,
    link: &ScannedPageLink,
    config: &SiteConfig,
) -> Option<UrlPath> {
    if !link.is_page_candidate() {
        return None;
    }

    match LinkKind::parse(&link.dest) {
        LinkKind::External(_) | LinkKind::Fragment(_) => None,
        LinkKind::SiteRoot(_) => {
            let permalink =
                crate::pipeline::transform::normalize_site_root_page_url(&link.dest, config);
            contains_page(store, &permalink).then_some(permalink)
        }
        LinkKind::FileRelative(path) => {
            let (path, _) = split_path_fragment(path);
            let source_dir = source_path.parent().unwrap_or(Path::new(""));
            let physical = crate::address::resolve_physical_path(source_dir, path);

            if let Some(permalink) = permalink_for_source_candidate(store, &physical) {
                return Some(permalink);
            }

            let permalink = resolve_relative_url(current_permalink, path);
            contains_page(store, &permalink).then_some(permalink)
        }
    }
}
