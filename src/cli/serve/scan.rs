//! Quick page scanning for progressive serving.
//!
//! Scans all content files, extracts metadata, and builds URL mappings
//! in `GLOBAL_ADDRESS_SPACE` and `STORED_PAGES` without full compilation.

use anyhow::Result;

use crate::compiler::page::{
    build_address_space, collect_content_files, filter_markdown_drafts, filter_typst_drafts,
};
use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::page::{CompiledPage, PAGE_LINKS, STORED_PAGES};

/// Scan all content files and populate global state.
///
/// This extracts metadata from all pages (via Typst batch_scan for .typ,
/// frontmatter parsing for .md) and populates:
/// - `GLOBAL_ADDRESS_SPACE`: URL ↔ Source mapping (with custom permalinks)
/// - `STORED_PAGES`: Page metadata for `@tola/pages` virtual package
/// - `PAGE_LINKS`: Internal link graph
///
/// Requires Typst to be initialized before calling.
pub fn scan_pages(config: &SiteConfig) -> Result<()> {
    STORED_PAGES.clear();
    PAGE_LINKS.clear();

    let content_files = collect_content_files(&config.build.content);
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    let root = config.get_root();
    let label = &config.build.meta.label;

    // Scan files to extract metadata (reuses filter_drafts logic)
    let typst_result = filter_typst_drafts(&typst_files, root, label);
    let md_result = filter_markdown_drafts(&markdown_files, root, label);

    let scanned = [typst_result.scanned, md_result.scanned].concat();
    let drafts_skipped = typst_result.draft_count + md_result.draft_count;

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

    // Mark scan as completed (build_all will skip redundant global state updates)
    crate::core::set_scan_completed();

    let total = pages.len();
    if drafts_skipped > 0 {
        crate::debug!("scan"; "registered {} pages ({} drafts skipped)", total, drafts_skipped);
    } else {
        crate::debug!("scan"; "registered {} pages", total);
    }

    Ok(())
}
