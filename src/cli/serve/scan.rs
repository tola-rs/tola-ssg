//! Quick page scanning for progressive serving.
//!
//! Scans all content files, extracts metadata, and builds URL mappings
//! in the site state's address space and page state without full compilation.

use anyhow::Result;

use crate::address::SiteIndex;
use crate::compiler::page::{build_address_space, collect_content_files};
use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::page::CompiledPage;

/// Scan all content files and populate runtime state
///
/// This extracts metadata from all pages (via Typst batch_scan for .typ,
/// frontmatter parsing for .md) and populates:
/// - Address space: URL ↔ Source mapping (with custom permalinks)
/// - page store: Page metadata for `@tola/pages` virtual package
/// - page link graph used by `@tola/current`
///
/// Requires Typst to be initialized before calling
pub fn scan_pages(config: &SiteConfig, state: &SiteIndex) -> Result<()> {
    let next = SiteIndex::new();

    let content_files = collect_content_files(&config.build.content);
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    let scan_result = crate::compiler::page::scan_pages(config, &typst_files, &markdown_files);

    // Report scan phase errors
    scan_result.report_errors(
        config.build.diagnostics.max_errors.unwrap_or(usize::MAX),
        config.get_root(),
    )?;

    let scanned = scan_result.scanned;
    let drafts_skipped = scan_result.drafts_skipped;

    // Build CompiledPage list with correct permalinks
    let pages: Vec<CompiledPage> = scanned
        .iter()
        .filter_map(|s| CompiledPage::from_paths_with_meta(&s.path, config, s.meta.clone()).ok())
        .collect();

    // Populate page metadata and link graph.
    next.with_pages(|store| crate::compiler::page::populate_pages(&scanned, config, store));

    // Populate address space.
    build_address_space(&pages, config, &next);
    state.replace_with(next);

    let total = pages.len();
    if drafts_skipped > 0 {
        crate::debug!("scan"; "registered {} pages ({} drafts skipped)", total, drafts_skipped);
    } else {
        crate::debug!("scan"; "registered {} pages", total);
    }

    Ok(())
}
