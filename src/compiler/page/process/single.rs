//! Single page compilation and metadata extraction.
//!
//! Handles compiling individual content files (Typst, Markdown) and extracting metadata.

use crate::compiler::CompileContext;
use crate::compiler::page::compile;
use crate::compiler::page::{
    PageResult, ScannedHeading, ScannedPageLink, SinglePageScanData, collect_warnings,
    scan_single_page,
};
use crate::config::SiteConfig;
use crate::core::{BuildMode, GLOBAL_ADDRESS_SPACE, UrlPath};
use crate::package::TolaPackage;
use crate::page::{CompiledPage, PAGE_LINKS, PageMeta, STORED_PAGES, StaleLinkPolicy};
use crate::utils::path::normalize_path;
use crate::utils::path::slug::slugify_fragment;
use anyhow::Result;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct PageStateEpoch {
    value: Arc<Mutex<u64>>,
}

impl PageStateEpoch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ticket(&self) -> PageStateTicket {
        PageStateTicket {
            epoch: self.clone(),
            value: *self.value.lock(),
        }
    }

    pub fn advance(&self) {
        *self.value.lock() += 1;
    }
}

#[derive(Clone)]
pub struct PageStateTicket {
    epoch: PageStateEpoch,
    value: u64,
}

impl PageStateTicket {
    pub(crate) fn commit<T>(&self, write: impl FnOnce() -> T) -> Option<T> {
        let current = self.epoch.value.lock();
        if *current != self.value {
            return None;
        }
        Some(write())
    }
}

enum PageStateCommit<'a> {
    Always,
    Ticket(&'a PageStateTicket),
}

impl PageStateCommit<'_> {
    fn commit(
        self,
        source: &Path,
        page: &CompiledPage,
        scan_data: &SinglePageScanData,
        config: &SiteConfig,
    ) -> bool {
        match self {
            Self::Always => {
                commit_page_state(source, page, scan_data, config);
                true
            }
            Self::Ticket(ticket) => ticket
                .commit(|| commit_page_state(source, page, scan_data, config))
                .is_some(),
        }
    }
}

fn record_scanned_links(
    source: &Path,
    permalink: &UrlPath,
    links: &[ScannedPageLink],
    config: &SiteConfig,
) {
    if links.is_empty() {
        PAGE_LINKS.record(permalink, vec![]);
        return;
    }

    let targets: Vec<_> = links
        .iter()
        .filter_map(|link| {
            crate::page::resolve_page_link_target(&STORED_PAGES, permalink, source, link, config)
        })
        .collect();
    PAGE_LINKS.record(permalink, targets);
}

fn heading_ids<'a>(
    headings: &'a [ScannedHeading],
    config: &'a SiteConfig,
) -> impl Iterator<Item = String> + 'a {
    headings
        .iter()
        .map(|heading| slugify_fragment(&heading.text, &config.build.slug))
}

fn relative_source_path(path: &Path, config: &SiteConfig) -> Option<String> {
    normalize_path(path)
        .strip_prefix(normalize_path(&config.build.content))
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

fn filename_from_relative(path: Option<&str>) -> Option<String> {
    path.and_then(|path| {
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
    })
}

fn pages_for_urls(urls: &[UrlPath]) -> Vec<crate::page::StoredPage> {
    let pages = STORED_PAGES.get_pages_with_drafts();
    urls.iter()
        .filter_map(|url| pages.iter().find(|page| page.permalink == *url).cloned())
        .collect()
}

fn current_context_from_scan(
    source: &Path,
    page: &CompiledPage,
    scan_data: &SinglePageScanData,
    config: &SiteConfig,
) -> serde_json::Value {
    let permalink = &page.route.permalink;
    let path = relative_source_path(source, config);
    let filename = filename_from_relative(path.as_deref());
    let parent = permalink.parent().map(|parent| parent.as_str().to_string());
    let links_to_urls: Vec<_> = scan_data
        .links
        .iter()
        .filter_map(|link| {
            crate::page::resolve_page_link_target(&STORED_PAGES, permalink, source, link, config)
        })
        .collect();
    let linked_by_urls = PAGE_LINKS.linked_by(permalink);

    serde_json::json!({
        TolaPackage::Current.input_key(): {
            "current-permalink": permalink.as_str(),
            "parent-permalink": parent,
            "path": path,
            "filename": filename,
            "links_to": pages_for_urls(&links_to_urls),
            "linked_by": pages_for_urls(&linked_by_urls),
            "headings": scan_data.headings,
        }
    })
}

fn commit_page_state(
    source: &Path,
    page: &CompiledPage,
    scan_data: &SinglePageScanData,
    config: &SiteConfig,
) {
    STORED_PAGES.sync_source_permalink(
        source,
        page.route.permalink.clone(),
        StaleLinkPolicy::Clear,
    );
    STORED_PAGES.insert_headings(page.route.permalink.clone(), scan_data.headings.clone());
    record_scanned_links(source, &page.route.permalink, &scan_data.links, config);

    STORED_PAGES.insert_page(
        page.route.permalink.clone(),
        page.content_meta.clone().unwrap_or_default(),
    );

    // Register headings for fragment validation.
    // Page route registration is deferred to the hot-reload routing layer so
    // permalink changes can still be detected from old vs new URL mappings.
    GLOBAL_ADDRESS_SPACE.write().register_headings(
        &page.route.permalink,
        heading_ids(&scan_data.headings, config),
    );
}

// ============================================================================
// Single Page Processing (watch mode)
// ============================================================================

/// Process a single content file (Typst or Markdown)
///
/// The mode controls:
/// - `emit_ids`: Whether to output `data-tola-id` attributes
/// - `cache_vdom`: Whether to return indexed VDOM for hot reload
///
/// Note: This function does NOT write the HTML file to disk
/// The caller should decide whether to write based on diff results
pub fn process_page(
    mode: BuildMode,
    path: &Path,
    config: &SiteConfig,
) -> Result<Option<PageResult>> {
    process_page_inner(mode, path, config, PageStateCommit::Always)
}

pub fn process_page_with_ticket(
    mode: BuildMode,
    path: &Path,
    config: &SiteConfig,
    ticket: &PageStateTicket,
) -> Result<Option<PageResult>> {
    process_page_inner(mode, path, config, PageStateCommit::Ticket(ticket))
}

fn process_page_inner(
    mode: BuildMode,
    path: &Path,
    config: &SiteConfig,
    commit: PageStateCommit<'_>,
) -> Result<Option<PageResult>> {
    let mut page = CompiledPage::from_paths(path, config)?;

    // Scan to extract headings and links ===
    // This must happen BEFORE compile so @tola/current has fresh data.
    // The scan result is passed as a local compile input; global stores are
    // committed only after compilation succeeds.
    let scan_data = scan_single_page(path, config);
    page.apply_meta(scan_data.meta.clone(), config);

    // Compile with fresh @tola/current data ===
    let current_context = current_context_from_scan(path, &page, &scan_data, config);
    let ctx = CompileContext::new(mode, config)
        .with_route(&page.route)
        .with_current_context(&current_context);
    let result = compile(path, &ctx)?;

    // Extract metadata
    let content_meta: Option<PageMeta> = result.meta;

    // Skip drafts
    if content_meta.as_ref().is_some_and(|m| m.draft) {
        return Ok(None);
    }

    // Record dependencies (thread-local for parallel safety)
    let mut deps = result.accessed_files;
    for pkg in &result.accessed_packages {
        if let Some(sentinel) = crate::package::package_sentinel(pkg) {
            deps.push(sentinel);
        }
    }
    crate::compiler::dependency::record_dependencies_local(path, deps);

    page.apply_meta(content_meta, config);
    page.compiled_html = Some(result.html);

    let warnings = result.warnings.clone();
    collect_warnings(&result.warnings);

    if !commit.commit(path, &page, &scan_data, config) {
        return Ok(None);
    }

    let permalink = page.route.permalink.clone();

    Ok(Some(PageResult {
        page,
        indexed_vdom: result.indexed_vdom,
        permalink,
        warnings,
    }))
}

// ============================================================================
// Metadata Extraction
// ============================================================================

/// Compile a content file (Typst or Markdown) and extract metadata
///
/// Also records dependencies for incremental rebuild tracking
/// Uses the VDOM pipeline for HTML generation
///
/// When using `DEVELOPMENT` mode, emits `data-tola-id` attributes
/// and returns indexed VDOM for caller to cache (decoupled from hotreload)
#[allow(dead_code)]
pub fn compile_meta(
    mode: BuildMode,
    path: &Path,
    config: &SiteConfig,
) -> Result<crate::compiler::page::CompileMetaResult> {
    // Build context without route - compile_meta is typically used for production
    // where globally unique StableIds aren't needed
    let ctx = CompileContext::new(mode, config);
    let result = compile(path, &ctx)?;

    let meta = result.meta;

    crate::compiler::dependency::record_dependencies_local(path, result.accessed_files);

    // Return indexed_vdom to caller for caching decision
    // (decouples compiler from hotreload)
    let indexed_vdom = result.indexed_vdom;

    let html = result.html;

    // Collect warnings after all other uses of result (to avoid partial move)
    collect_warnings(&result.warnings);

    Ok((html, meta, indexed_vdom))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::UrlPath;
    use std::fs;
    use tempfile::TempDir;

    fn reset_global_state() {
        STORED_PAGES.clear();
        PAGE_LINKS.clear();
        GLOBAL_ADDRESS_SPACE.write().clear();
    }

    #[test]
    fn test_compile_meta_no_label() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");

        // File without <tola-meta> label
        fs::write(&file_path, "= Hello World").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());

        let result = compile_meta(BuildMode::DEVELOPMENT, &file_path, &config);
        assert!(result.is_ok(), "compile_meta should succeed: {:?}", result);

        let (html, meta, _indexed_vdom) = result.unwrap();
        assert!(!html.is_empty(), "HTML should not be empty");
        assert!(
            meta.is_none(),
            "Metadata should be None when no <tola-meta> label"
        );
    }

    #[test]
    fn test_compile_meta_with_label() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");

        fs::write(
            &file_path,
            r#"#metadata((
  title: "Test",
  author: "Author",
)) <tola-meta>

= Content
"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());

        let result = compile_meta(BuildMode::DEVELOPMENT, &file_path, &config);
        assert!(result.is_ok(), "compile_meta should succeed: {:?}", result);

        let (html, meta, _indexed_vdom) = result.unwrap();
        assert!(!html.is_empty());
        assert!(meta.is_some());

        let meta = meta.unwrap();
        assert_eq!(meta.title, Some("Test".to_string()));
        assert_eq!(meta.author, Some("Author".to_string()));
    }

    #[test]
    fn test_compile_meta_draft_field() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");

        fs::write(
            &file_path,
            r#"#metadata((
  title: "Draft Post",
  draft: true,
)) <tola-meta>

= Draft
"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());

        let result = compile_meta(BuildMode::DEVELOPMENT, &file_path, &config);
        assert!(result.is_ok());

        let (_, meta, _indexed_vdom) = result.unwrap();
        assert!(meta.is_some());
        assert!(
            meta.as_ref().is_some_and(|m| m.draft),
            "Should detect draft: true"
        );
    }

    #[test]
    fn test_compile_error_returns_err() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("invalid.typ");

        // Create an invalid typst file
        fs::write(&file_path, "#invalid-syntax-that-will-fail").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());

        let result = compile_meta(BuildMode::DEVELOPMENT, &file_path, &config);

        // Should return an error, not panic or silently skip
        assert!(result.is_err(), "Invalid typst should return Err");
    }

    #[test]
    fn test_process_page_error_does_not_commit_page_state() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file_path = content_dir.join("invalid.typ");

        fs::write(&file_path, "#invalid-syntax-that-will-fail").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        reset_global_state();

        let result = process_page(BuildMode::DEVELOPMENT, &file_path, &config);

        assert!(result.is_err(), "invalid page should fail compilation");
        assert!(
            STORED_PAGES.get_permalink_by_source(&file_path).is_none(),
            "failed compile must not publish source permalink mapping"
        );
        assert!(
            STORED_PAGES.get_pages_with_drafts().is_empty(),
            "failed compile must not publish page metadata"
        );
        assert!(
            GLOBAL_ADDRESS_SPACE.read().is_empty(),
            "failed compile must not publish address-space state"
        );

        reset_global_state();
    }

    #[test]
    fn test_process_page_error_keeps_existing_page_state() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file_path = content_dir.join("invalid.typ");

        fs::write(&file_path, "#invalid-syntax-that-will-fail").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        reset_global_state();

        let old_permalink = UrlPath::from_page("/already-visible/");
        STORED_PAGES.insert_page(
            old_permalink.clone(),
            PageMeta {
                title: Some("Already Visible".to_string()),
                ..Default::default()
            },
        );
        STORED_PAGES.insert_source_mapping(file_path.clone(), old_permalink.clone());
        STORED_PAGES.insert_headings(
            old_permalink.clone(),
            vec![ScannedHeading {
                level: 1,
                text: "Old Heading".to_string(),
                supplement: None,
            }],
        );

        let result = process_page(BuildMode::DEVELOPMENT, &file_path, &config);

        assert!(result.is_err(), "invalid page should fail compilation");
        assert_eq!(
            STORED_PAGES.get_permalink_by_source(&file_path),
            Some(old_permalink.clone()),
            "failed recompile must keep existing source permalink mapping"
        );
        assert!(
            STORED_PAGES
                .get_pages_with_drafts()
                .iter()
                .any(|page| page.permalink == old_permalink),
            "failed recompile must keep existing page metadata"
        );
        assert_eq!(STORED_PAGES.get_headings(&old_permalink).len(), 1);

        reset_global_state();
    }

    #[test]
    fn test_process_page_removes_stale_permalink_for_same_source() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file_path = content_dir.join("source-field-test.md");

        // TOML frontmatter: ensure `permalink` is parsed by PageMeta.
        fs::write(
            &file_path,
            r#"+++
title = "Testing @tola/current.path"
date = "datetime(year: 2026, month: 2, day: 25)"
permalink = "/showcase/source-field-test/"
+++

# Body
"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        reset_global_state();

        let old_permalink = UrlPath::from_page("/showcase/2026_02_25_source-field-test/");
        STORED_PAGES.insert_page(
            old_permalink.clone(),
            PageMeta {
                title: Some("Testing @tola/current.path".to_string()),
                ..Default::default()
            },
        );
        STORED_PAGES.insert_source_mapping(file_path.clone(), old_permalink.clone());

        let result = process_page(BuildMode::DEVELOPMENT, &file_path, &config)
            .expect("process_page should succeed");
        assert!(result.is_some(), "page should not be filtered as draft");

        let mapped = STORED_PAGES.get_permalink_by_source(&file_path);
        assert!(mapped.is_some(), "source mapping should exist");
        assert_ne!(
            mapped,
            Some(old_permalink.clone()),
            "source mapping should be updated from old permalink"
        );

        let pages = STORED_PAGES.get_pages_with_drafts();
        let mapped = mapped.unwrap();
        assert!(
            pages.iter().any(|p| p.permalink == mapped),
            "mapped permalink should exist"
        );
        assert!(
            !pages.iter().any(|p| p.permalink == old_permalink),
            "old permalink should be removed"
        );

        reset_global_state();
    }

    #[test]
    fn test_process_page_uses_scanned_permalink_for_current_context_and_headings() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file_path = content_dir.join("post.typ");

        fs::write(
            &file_path,
            r#"#metadata((
  title: "Custom Permalink",
  permalink: "/notes/custom/"
)) <tola-meta>

= Hello World
"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        reset_global_state();

        let result = process_page(BuildMode::DEVELOPMENT, &file_path, &config)
            .expect("process_page should succeed")
            .expect("page should not be filtered");

        assert_eq!(result.permalink, UrlPath::from_page("/notes/custom/"));
        assert_eq!(
            STORED_PAGES.get_permalink_by_source(&file_path),
            Some(UrlPath::from_page("/notes/custom/"))
        );

        let headings = GLOBAL_ADDRESS_SPACE
            .read()
            .headings_for(&UrlPath::from_page("/notes/custom/"))
            .cloned()
            .unwrap_or_default();
        assert!(headings.contains("hello-world"));

        reset_global_state();
    }

    #[test]
    fn test_process_page_with_stale_ticket_does_not_commit_page_state() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file_path = content_dir.join("post.md");

        fs::write(&file_path, "+++\ntitle = \"Post\"\n+++\n\n# Post\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        reset_global_state();

        let epoch = PageStateEpoch::new();
        let ticket = epoch.ticket();
        epoch.advance();

        let result = process_page_with_ticket(BuildMode::DEVELOPMENT, &file_path, &config, &ticket)
            .expect("stale page compile should not fail");

        assert!(result.is_none(), "stale page compile must be discarded");
        assert!(
            STORED_PAGES.get_permalink_by_source(&file_path).is_none(),
            "stale page compile must not publish source permalink mapping"
        );
        assert!(
            STORED_PAGES.get_pages_with_drafts().is_empty(),
            "stale page compile must not publish page metadata"
        );

        reset_global_state();
    }
}
