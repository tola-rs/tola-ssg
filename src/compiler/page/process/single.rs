//! Single page compilation and metadata extraction.
//!
//! Handles compiling individual content files (Typst, Markdown) and extracting metadata.

use crate::compiler::CompileContext;
use crate::compiler::page::compile;
use crate::compiler::page::{PageResult, collect_warnings};
use crate::config::SiteConfig;
use crate::core::BuildMode;
use crate::page::{STORED_PAGES, PageMeta, CompiledPage};
use anyhow::Result;
use std::path::Path;

// ============================================================================
// Single Page Processing (watch mode)
// ============================================================================

/// Process a single content file (Typst or Markdown).
///
/// The mode controls:
/// - `emit_ids`: Whether to output `data-tola-id` attributes
/// - `cache_vdom`: Whether to return indexed VDOM for hot reload
///
/// Note: This function does NOT write the HTML file to disk.
/// The caller should decide whether to write based on diff results.
pub fn process_page(
    mode: BuildMode,
    path: &Path,
    config: &SiteConfig,
) -> Result<Option<PageResult>> {
    let mut page = CompiledPage::from_paths(path, config)?;

    // Build compile context with route for StableId seeding and link resolution
    let ctx = CompileContext::new(mode, config).with_route(&page.route);

    // Compile using unified compile (supports Typst + Markdown)
    let result = compile(path, &ctx)?;

    // Extract metadata
    let content_meta: Option<PageMeta> = result.meta;

    // Skip drafts
    if content_meta.as_ref().is_some_and(|m| m.draft) {
        return Ok(None);
    }

    // Record dependencies (thread-local for parallel safety)
    // Include virtual package sentinels for @tola/* packages
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

    collect_warnings(&result.warnings);

    // Update global site data with permalink and content metadata
    STORED_PAGES.insert_page(
        page.route.permalink.clone(),
        page.content_meta.clone().unwrap_or_default(),
    );

    let permalink = page.route.permalink.clone();

    Ok(Some(PageResult {
        page,
        indexed_vdom: result.indexed_vdom,
        permalink,
    }))
}

// ============================================================================
// Metadata Extraction
// ============================================================================

/// Compile a content file (Typst or Markdown) and extract metadata.
///
/// Also records dependencies for incremental rebuild tracking.
/// Uses the VDOM pipeline for HTML generation.
///
/// When using `DEVELOPMENT` mode, emits `data-tola-id` attributes
/// and returns indexed VDOM for caller to cache (decoupled from hotreload).
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
    use std::fs;
    use tempfile::TempDir;

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
        assert!(meta.as_ref().is_some_and(|m| m.draft), "Should detect draft: true");
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
}
