//! Single page compilation and metadata extraction.
//!
//! Handles compiling individual content files (Typst, Markdown) and extracting metadata.

use crate::compiler::CompileContext;
use crate::compiler::page::compile;
use crate::compiler::page::{PageResult, collect_warnings, scan_single_page};
use crate::config::SiteConfig;
use crate::core::{BuildMode, GLOBAL_ADDRESS_SPACE};
use crate::page::{CompiledPage, PAGE_LINKS, PageMeta, STORED_PAGES};
use crate::utils::path::slug::slugify_path;
use anyhow::Result;
use std::path::Path;

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
    let mut page = CompiledPage::from_paths(path, config)?;

    // Scan to extract headings and links ===
    // This must happen BEFORE compile so @tola/current has fresh data
    let scan_data = scan_single_page(path, config);

    // Update headings and links in global stores
    let permalink = page.route.permalink.clone();
    STORED_PAGES.insert_headings(permalink.clone(), scan_data.headings);

    // Convert raw link paths to UrlPaths and record in PAGE_LINKS
    if !scan_data.links.is_empty() {
        let slug_config = &config.build.slug;
        let targets: Vec<_> = scan_data
            .links
            .iter()
            .map(|link| {
                let (path, _) = crate::utils::path::route::split_path_fragment(link);
                let slugified = slugify_path(path, slug_config);
                crate::core::UrlPath::from_page(&format!(
                    "/{}/",
                    slugified.to_string_lossy().trim_matches('/')
                ))
            })
            .collect();
        PAGE_LINKS.record(&permalink, targets);
    }

    // Compile with fresh @tola/current data ===
    let ctx = CompileContext::new(mode, config).with_route(&page.route);
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

    page.content_meta = content_meta;
    page.apply_custom_permalink(config);
    page.compiled_html = Some(result.html);

    let warnings = result.warnings.clone();
    collect_warnings(&result.warnings);

    // Permalink can change after compile (e.g. custom permalink from metadata).
    // Remove stale entry for this source to avoid duplicate pages in @tola/pages.
    if let Some(old_permalink) = STORED_PAGES.get_permalink_by_source(path)
        && old_permalink != page.route.permalink
    {
        STORED_PAGES.remove_page(&old_permalink);
        PAGE_LINKS.record(&old_permalink, vec![]);
    }

    // Update global site data
    STORED_PAGES.insert_page(
        page.route.permalink.clone(),
        page.content_meta.clone().unwrap_or_default(),
    );
    STORED_PAGES.insert_source_mapping(path.to_path_buf(), page.route.permalink.clone());

    // Register to AddressSpace for hot-reload tracking
    GLOBAL_ADDRESS_SPACE.write().register_page(
        page.route.clone(),
        page.content_meta.as_ref().and_then(|m| m.title.clone()),
    );

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
    fn test_process_page_removes_stale_permalink_for_same_source() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file_path = content_dir.join("source-field-test.md");

        // TOML frontmatter: ensure `permalink` is parsed by PageMeta.
        fs::write(
            &file_path,
            r#"+++
title = "Testing @tola/current.source"
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
                title: Some("Testing @tola/current.source".to_string()),
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
}
