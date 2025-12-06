use crate::compiler::meta::{PageMeta, ContentMeta, Pages};
use crate::compiler::{collect_all_files, is_up_to_date};
use crate::utils::minify::{minify, MinifyType};
use crate::utils::xml::process_html;
use crate::utils::exec::FilterRule;
use crate::{config::SiteConfig, exec, log, typst_lib};
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::time::SystemTime;
use tempfile::Builder as TempFileBuilder;
use rayon::prelude::*;

/// Skip known HTML export warnings (used by `compile_cli`).
const TYPST_FILTER: FilterRule = FilterRule::new(&[
    "warning: html export is under active development",
    "and incomplete",
    "= hint: its behaviour may change at any time",
    "= hint: do not rely on this feature for production use cases",
    "= hint: see https://github.com/typst/typst/issues/5512",
    "for more information",
    "warning: elem",
]);

/// Label name for querying typst metadata
const TOLA_META_LABEL: &str = "tola-meta";

// ============================================================================
// Public API
// ============================================================================

/// Compile all pages and write HTML files.
///
/// # Behavior
///
/// - **Lib mode**: Uses pre-compiled HTML from `page.compiled_html` (set by `collect_pages`).
/// - **CLI mode**: Compiles each page using `typst compile` CLI.
///
/// Skips pages that are up-to-date (unless `clean` is true).
/// Calls `on_progress` after each page is processed.
pub fn compile_pages(
    pages: &Pages,
    config: &'static SiteConfig,
    clean: bool,
    deps_mtime: Option<SystemTime>,
    on_progress: impl Fn() + Sync,
) -> Result<()> {
    pages.items.par_iter().try_for_each(|page| {
        let result = write_page(page, config, clean, deps_mtime, false);
        on_progress();
        result
    })
}

// ============================================================================
// process_page: Single page compilation (for watch mode)
// ============================================================================

/// Process a single .typ file: compile and write HTML.
///
/// Used by watch mode to compile individual files on change.
/// Returns `Some(PageMeta)` if compiled, `None` if skipped (up-to-date or draft).
pub fn process_page(
    path: &Path,
    config: &'static SiteConfig,
    clean: bool,
    deps_mtime: Option<SystemTime>,
    log_file: bool,
) -> Result<Option<PageMeta>> {
    let mut page = PageMeta::from_paths(path.to_path_buf(), config)?;

    // Check if up-to-date
    if !clean && is_up_to_date(path, &page.paths.html, deps_mtime) {
        return Ok(None);
    }

    // Compile and get HTML + metadata
    let (html_content, content_meta) = compile_with_meta(path, config)?;

    // Skip drafts
    if is_draft(&content_meta) {
        return Ok(None);
    }

    page.content_meta = content_meta;
    page.compiled_html = Some(html_content);

    // Write the page
    write_page(&page, config, true, None, log_file)?;

    Ok(Some(page))
}

// ============================================================================
// Internal
// ============================================================================

/// Write a page's HTML to disk.
fn write_page(
    page: &PageMeta,
    config: &'static SiteConfig,
    clean: bool,
    deps_mtime: Option<SystemTime>,
    log_file: bool,
) -> Result<()> {
    // Check if up-to-date (only for batch mode, process_page already checked)
    if !clean && is_up_to_date(&page.paths.source, &page.paths.html, deps_mtime) {
        return Ok(());
    }

    if log_file {
        log!("content"; "{}", page.paths.relative);
    }

    // Create output directory
    if let Some(parent) = page.paths.html.parent() {
        fs::create_dir_all(parent)?;
    }

    // Get HTML content
    let html_content = if let Some(ref html) = page.compiled_html {
        html.clone()
    } else {
        // CLI mode in batch: compile now
        compile_cli(&page.paths.source, config)?
    };

    // Post-process and write
    let html_content = process_html(&page.paths.html, &html_content, config)?;
    let html_content = minify(MinifyType::Html(&html_content), config);
    fs::write(&page.paths.html, &*html_content)?;

    Ok(())
}

/// Compile a typst file and extract metadata (lib or CLI mode).
pub fn compile_with_meta(path: &Path, config: &SiteConfig) -> Result<(Vec<u8>, Option<ContentMeta>)> {
    if config.build.typst.use_lib {
        let root = config.get_root();
        let result = typst_lib::compile_with_metadata(path, root, TOLA_META_LABEL)?;
        let meta = result.metadata.and_then(|json| serde_json::from_value(json).ok());
        Ok((result.html, meta))
    } else {
        let meta = query_meta(path, config);
        let html = compile_cli(path, config)?;
        Ok((html, meta))
    }
}

/// Query metadata only (lib or CLI mode).
pub fn query_meta(path: &Path, config: &SiteConfig) -> Option<ContentMeta> {
    if config.build.typst.use_lib {
        let root = config.get_root();
        let result = typst_lib::compile_with_metadata(path, root, TOLA_META_LABEL).ok()?;
        result.metadata.and_then(|json| serde_json::from_value(json).ok())
    } else {
        query_meta_cli(path, config)
    }
}

// ============================================================================
// Internal: CLI helpers
// ============================================================================

/// Compile using typst CLI.
fn compile_cli(source: &Path, config: &SiteConfig) -> Result<Vec<u8>> {
    let root = config.get_root();
    let temp_file = TempFileBuilder::new()
        .prefix("tola_typst_")
        .suffix(".html")
        .tempfile()?;

    exec!(
        pty=true;
        filter=&TYPST_FILTER;
        &config.build.typst.command;
        "compile", "--features", "html", "--format", "html",
        "--font-path", root, "--root", root,
        source, temp_file.path()
    )?;

    Ok(fs::read(temp_file.path())?)
}

/// Query metadata using typst CLI.
fn query_meta_cli(path: &Path, config: &SiteConfig) -> Option<ContentMeta> {
    use crate::utils::exec::SILENT_FILTER;
    let root = config.get_root();

    let output = exec!(
        filter=&SILENT_FILTER;
        &config.build.typst.command;
        "query", "--features", "html", "--format", "json",
        "--font-path", root, "--root", root,
        path, "<tola-meta>", "--field", "value", "--one"
    );

    output.ok().and_then(|out| {
        let json_str = std::str::from_utf8(&out.stdout).ok()?;
        serde_json::from_str(json_str).ok()
    })
}

/// Check if content metadata indicates a draft.
#[inline]
fn is_draft(meta: &Option<ContentMeta>) -> bool {
    meta.as_ref().is_some_and(|c| c.draft)
}

/// Collect all pages from content directory with metadata.
///
/// This function scans the content directory for `.typ` files and collects
/// their metadata using either lib mode or CLI mode based on config.
///
/// # Behavior
///
/// - **Lib mode**: Compiles each file once, extracting both HTML and metadata.
///   The compiled HTML is stored in `PageMeta.compiled_html` for later use.
/// - **CLI mode**: Only queries metadata (no compilation yet).
///   `PageMeta.compiled_html` will be `None`.
///
/// Draft pages (with `draft: true` in metadata) are automatically filtered out.
/// Pages without `<tola-meta>` are still built (content_meta = None).
pub fn collect_pages(config: &'static SiteConfig) -> Result<Pages> {
    let content_files = collect_all_files(&config.build.content);

    let typ_files: Vec<_> = content_files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "typ"))
        .collect();

    let results: Vec<Result<Option<PageMeta>>> = typ_files
        .par_iter()
        .map(|path| {
            let mut page = PageMeta::from_paths(path.clone(), config)?;

            let (html, content_meta) = if config.build.typst.use_lib {
                // Lib mode: compile once, get both HTML and metadata (metadata may be None)
                let result = compile_with_meta(path, config)?;
                (Some(result.0), result.1)
            } else {
                // CLI mode: only query metadata, compile later (metadata may be None)
                (None, query_meta(path, config))
            };

            // Skip drafts
            if is_draft(&content_meta) {
                return Ok(None);
            }

            page.content_meta = content_meta;
            page.compiled_html = html;
            Ok(Some(page))
        })
        .collect();

    // Check for errors and collect successful pages
    let mut items = Vec::with_capacity(results.len());
    for result in results {
        match result {
            Ok(Some(page)) => items.push(page),
            Ok(None) => {} // Draft, skip
            Err(e) => return Err(e), // Propagate compilation error
        }
    }

    Ok(Pages { items })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Create a test config with the given content directory.
    fn make_test_config(content_dir: PathBuf, output_dir: PathBuf) -> &'static SiteConfig {
        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = output_dir;
        config.build.typst.use_lib = true;
        Box::leak(Box::new(config))
    }

    #[test]
    fn test_is_draft_none() {
        assert!(!is_draft(&None));
    }

    #[test]
    fn test_is_draft_false() {
        let meta = ContentMeta {
            draft: false,
            ..Default::default()
        };
        assert!(!is_draft(&Some(meta)));
    }

    #[test]
    fn test_is_draft_true() {
        let meta = ContentMeta {
            draft: true,
            ..Default::default()
        };
        assert!(is_draft(&Some(meta)));
    }

    // Tests that use typst compilation

    #[test]
    fn test_compile_with_meta_no_label() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");

        // File without <tola-meta> label
        fs::write(&file_path, "= Hello World").unwrap();

        let mut config = SiteConfig::default();
        config.build.typst.use_lib = true;
        config.set_root(dir.path());

        let result = compile_with_meta(&file_path, &config);
        assert!(result.is_ok(), "compile_with_meta should succeed: {:?}", result);

        let (html, meta) = result.unwrap();
        assert!(!html.is_empty(), "HTML should not be empty");
        assert!(meta.is_none(), "Metadata should be None when no <tola-meta> label");
    }

    #[test]
    fn test_compile_with_meta_with_label() {
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
        config.build.typst.use_lib = true;
        config.set_root(dir.path());

        let result = compile_with_meta(&file_path, &config);
        assert!(result.is_ok(), "compile_with_meta should succeed: {:?}", result);

        let (html, meta) = result.unwrap();
        assert!(!html.is_empty());
        assert!(meta.is_some());

        let meta = meta.unwrap();
        assert_eq!(meta.title, Some("Test".to_string()));
        assert_eq!(meta.author, Some("Author".to_string()));
    }

    #[test]
    fn test_compile_with_meta_draft_field() {
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
        config.build.typst.use_lib = true;
        config.set_root(dir.path());

        let result = compile_with_meta(&file_path, &config);
        assert!(result.is_ok());

        let (_, meta) = result.unwrap();
        assert!(meta.is_some());
        assert!(is_draft(&meta), "Should detect draft: true");
    }

    #[test]
    fn test_compile_error_returns_err() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("invalid.typ");

        // Create an invalid typst file
        fs::write(&file_path, "#invalid-syntax-that-will-fail").unwrap();

        let mut config = SiteConfig::default();
        config.build.typst.use_lib = true;
        config.set_root(dir.path());

        let result = compile_with_meta(&file_path, &config);

        // Should return an error, not panic or silently skip
        assert!(result.is_err(), "Invalid typst should return Err");
    }

    #[test]
    fn test_page_meta_from_paths() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        // Create a dummy file
        let file_path = content_dir.join("test.typ");
        fs::write(&file_path, "= Test").unwrap();

        let config = make_test_config(content_dir.clone(), output_dir);
        let page = PageMeta::from_paths(file_path, config);

        assert!(page.is_ok());
        let page = page.unwrap();
        assert_eq!(page.paths.relative, "test");
        assert!(page.paths.html.ends_with("test/index.html"));
    }

    #[test]
    fn test_page_meta_nested_path() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(content_dir.join("posts")).unwrap();

        let file_path = content_dir.join("posts/hello.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let config = make_test_config(content_dir.clone(), output_dir);
        let page = PageMeta::from_paths(file_path, config);

        assert!(page.is_ok());
        let page = page.unwrap();
        assert_eq!(page.paths.relative, "posts/hello");
    }
}
