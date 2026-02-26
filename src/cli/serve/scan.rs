//! Quick page scanning for progressive serving.
//!
//! Scans all content files, extracts metadata, and builds URL mappings
//! in `GLOBAL_ADDRESS_SPACE` and `STORED_PAGES` without full compilation.

use anyhow::Result;

use crate::compiler::page::{build_address_space, collect_content_files, filter_drafts};
use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::page::{CompiledPage, PAGE_LINKS, STORED_PAGES};

/// Scan all content files and populate global state
///
/// This extracts metadata from all pages (via Typst batch_scan for .typ,
/// frontmatter parsing for .md) and populates:
/// - `GLOBAL_ADDRESS_SPACE`: URL ↔ Source mapping (with custom permalinks)
/// - `STORED_PAGES`: Page metadata for `@tola/pages` virtual package
/// - `PAGE_LINKS`: Internal link graph
///
/// Requires Typst to be initialized before calling
pub fn scan_pages(config: &SiteConfig) -> Result<()> {
    STORED_PAGES.clear();
    PAGE_LINKS.clear();

    let content_files = collect_content_files(&config.build.content);
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    // Scan files to extract metadata (unified filter_drafts)
    let scan_result = filter_drafts(config, &typst_files, &markdown_files);

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
        .filter_map(|s| {
            let mut page = CompiledPage::from_paths(&s.path, config).ok()?;
            page.content_meta = s.meta.clone();
            page.apply_custom_permalink(config);
            Some(page)
        })
        .collect();

    // Populate STORED_PAGES and PAGE_LINKS
    crate::compiler::page::populate_pages(&scanned, config);

    // Populate GLOBAL_ADDRESS_SPACE
    build_address_space(&pages, config);

    let total = pages.len();
    if drafts_skipped > 0 {
        crate::debug!("scan"; "registered {} pages ({} drafts skipped)", total, drafts_skipped);
    } else {
        crate::debug!("scan"; "registered {} pages", total);
    }

    Ok(())
}
