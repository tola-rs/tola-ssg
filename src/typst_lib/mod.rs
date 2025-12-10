//! Typst library integration for direct compilation without CLI overhead.
//!
//! This module provides a high-performance alternative to invoking the `typst` CLI,
//! reducing compilation overhead by ~30% through resource sharing and caching.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                       Global Shared Resources                           │
//! │           (initialized once at startup, shared across ALL files)        │
//! ├─────────────┬─────────────┬─────────────────┬───────────────────────────┤
//! │ GLOBAL_FONTS│GLOBAL_LIBRARY│GLOBAL_PACKAGE   │   GLOBAL_FILE_CACHE       │
//! │ (~100ms)    │ (std lib)    │ (pkg cache)     │ (source/bytes per FileId) │
//! └──────┬──────┴──────┬───────┴────────┬────────┴─────────────┬────────────┘
//!        │             │                │                      │
//!        └─────────────┴────────────────┴──────────────────────┘
//!                                 │
//!                                 ▼
//!          ┌─────────────────────────────────────────┐
//!          │        SystemWorld (per-file, ~free)    │
//!          │  - Only stores: root, main, fonts ref   │
//!          │  - All caching via global statics       │
//!          └─────────────────────────────────────────┘
//! ```
//!
//! # Module Structure
//!
//! - [`font`] - Global shared font management
//! - [`package`] - Global shared package storage
//! - [`library`] - Global shared Typst standard library
//! - [`file`] - Global file cache with fingerprint-based invalidation
//! - [`world`] - `SystemWorld` implementation of the `World` trait
//! - [`diagnostic`] - Human-readable diagnostic formatting
//!
//! # Usage Example
//!
//! ```ignore
//! use tola::typst_lib;
//!
//! // Pre-warm at startup (optional but recommended)
//! typst_lib::warmup_with_root(Path::new("/project/root"));
//!
//! // Compile files - template.typ is cached after first use!
//! let result = typst_lib::compile_meta(
//!     Path::new("/project/content/page.typ"),
//!     Path::new("/project"),
//!     "tola-meta",
//! )?;
//! ```

mod diagnostic;
mod file;
mod font;
mod library;
mod package;
mod world;

use std::path::{Path, PathBuf};

use typst::foundations::{Label, Selector, Value};
use typst::introspection::MetadataElem;
use typst::utils::PicoStr;
use typst::Document;

pub use world::SystemWorld;

// =============================================================================
// Types
// =============================================================================

/// Compilation result containing HTML output and optional metadata.
#[derive(Debug)]
pub struct CompileResult {
    /// The compiled HTML content as bytes.
    pub html: Vec<u8>,
    /// Optional metadata value (None if label not found).
    pub metadata: Option<serde_json::Value>,
    /// Files accessed during compilation (for dependency tracking).
    /// This includes templates, utilities, and other imported files.
    pub accessed_files: Vec<PathBuf>,
}

// =============================================================================
// Test Synchronization
// =============================================================================

/// Test-only mutex to serialize typst compilations.
///
/// Typst's comemo caching can race in parallel test execution.
/// In production, rayon parallel compilation works fine.
#[cfg(test)]
static COMPILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Dummy guard for non-test mode (zero-cost).
#[cfg(not(test))]
struct DummyGuard;

/// Acquire compilation lock in test mode, no-op in production.
#[cfg(test)]
fn acquire_test_lock() -> std::sync::MutexGuard<'static, ()> {
    COMPILE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(not(test))]
const fn acquire_test_lock() -> DummyGuard {
    DummyGuard
}

// =============================================================================
// Public API
// =============================================================================

/// Pre-warm global resources (fonts, library, package storage).
///
/// Call once at startup to avoid lazy initialization during compilation.
/// Pass the project root to include custom fonts from the project directory.
pub fn warmup_with_root(root: &Path) {
    let _ = font::get_fonts(Some(root));
    let _ = &*library::GLOBAL_LIBRARY;
    let _ = &*package::GLOBAL_PACKAGE_STORAGE;
    let _ = &*file::GLOBAL_FILE_CACHE;
}

/// Compile a Typst file and extract metadata in a single pass.
///
/// This is the **recommended** entry point for building sites, avoiding
/// duplicate compilations for HTML and metadata.
///
/// # Arguments
///
/// * `path` - Path to the `.typ` file to compile
/// * `root` - Project root directory for resolving imports
/// * `label_name` - The label to query for metadata (e.g., "tola-meta")
pub fn compile_meta(
    path: &Path,
    root: &Path,
    label_name: &str,
) -> anyhow::Result<CompileResult> {
    let _guard = acquire_test_lock();
    let (_world, document) = compile_base(path, root)?;

    // Collect accessed files for dependency tracking
    let accessed_files = collect_accessed_files(root);

    let html = typst_html::html(&document)
        .map_err(|e| anyhow::anyhow!("HTML export failed: {e:?}"))?
        .into_bytes();

    let metadata = extract_meta(&document, label_name);

    Ok(CompileResult {
        html,
        metadata,
        accessed_files,
    })
}

// =============================================================================
// Internal Helpers
// =============================================================================

/// Core compilation logic shared by all entry points.
fn compile_base(
    path: &Path,
    root: &Path,
) -> anyhow::Result<(SystemWorld, typst_html::HtmlDocument)> {
    file::reset_access_flags();

    let world = SystemWorld::new(path, root);
    let result = typst::compile(&world);

    // Check for errors in warnings
    if diagnostic::has_errors(&result.warnings) {
        let formatted = diagnostic::format_diagnostics(&world, &result.warnings);
        anyhow::bail!("Typst compilation warnings:\n{formatted}");
    }

    // Extract document or format errors
    let document = result.output.map_err(|errors| {
        let all_diags: Vec<_> = errors.iter().chain(&result.warnings).cloned().collect();
        let filtered = diagnostic::filter_html_warnings(&all_diags);
        let formatted = diagnostic::format_diagnostics(&world, &filtered);
        anyhow::anyhow!("Typst compilation failed:\n{formatted}")
    })?;

    Ok((world, document))
}

/// Extract metadata from a compiled document by label name.
fn extract_meta(document: &typst_html::HtmlDocument, label_name: &str) -> Option<serde_json::Value> {
    let label = Label::new(PicoStr::intern(label_name))?;
    let introspector = document.introspector();
    let elem = introspector.query_unique(&Selector::Label(label)).ok()?;

    elem.to_packed::<MetadataElem>()
        .and_then(|meta| serde_json::to_value(&meta.value).ok())
}

/// Collect files accessed during the last compilation.
///
/// Returns paths of all local files (not packages) that were accessed.
/// Used for dependency tracking in incremental builds.
/// Includes virtual data files (`/_data/*.json`) for proper dependency tracking.
fn collect_accessed_files(root: &Path) -> Vec<std::path::PathBuf> {
    file::get_accessed_files()
        .into_iter()
        .filter(|id| id.package().is_none()) // Skip package files
        .filter_map(|id| {
            // Try to resolve to real path first
            id.vpath().resolve(root).or_else(|| {
                // For virtual files (like /_data/*.json), use the vpath directly
                let vpath = id.vpath().as_rooted_path();
                if crate::data::is_virtual_data_path(vpath) {
                    Some(vpath.to_path_buf())
                } else {
                    None
                }
            })
        })
        .collect()
}

/// Query metadata from a Typst file by label name.
///
/// Equivalent to `typst query <file> "<label>" --field value --one`.
#[allow(dead_code)]
pub fn query_meta(path: &Path, root: &Path, label_name: &str) -> anyhow::Result<Value> {
    let _guard = acquire_test_lock();
    let (_world, document) = compile_base(path, root)?;

    let label = Label::new(PicoStr::intern(label_name))
        .ok_or_else(|| anyhow::anyhow!("Invalid label name: {label_name}"))?;

    let introspector = document.introspector();
    let elem = introspector
        .query_unique(&Selector::Label(label))
        .map_err(|e| anyhow::anyhow!("Query failed: {e}"))?;

    elem.to_packed::<MetadataElem>()
        .map(|meta| meta.value.clone())
        .ok_or_else(|| anyhow::anyhow!("Element is not a metadata element"))
}

/// Disable ANSI color output for tests that check string content.
#[cfg(test)]
pub fn disable_colors() {
    colored::control::set_override(false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::meta::TOLA_META_LABEL;
    use std::fs;
    use tempfile::TempDir;

    /// Create a temporary project with a simple typst file.
    fn create_test_project() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let file_path = content_dir.join("test.typ");
        fs::write(&file_path, "= Hello World\n\nThis is a test.").unwrap();

        (dir, file_path)
    }

    #[test]
    fn test_warmup_does_not_panic() {
        let dir = TempDir::new().unwrap();
        // Should not panic even with empty directory
        warmup_with_root(dir.path());
    }

    #[test]
    fn test_compile_simple_document() {
        let (dir, file_path) = create_test_project();

        let result = compile_meta(&file_path, dir.path(), TOLA_META_LABEL);
        assert!(result.is_ok(), "Compilation should succeed: {:?}", result);

        let compiled = result.unwrap();
        let html = String::from_utf8_lossy(&compiled.html);
        assert!(html.contains("Hello World"), "HTML should contain heading");
    }

    #[test]
    fn test_compile_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let fake_path = dir.path().join("nonexistent.typ");

        let result = compile_meta(&fake_path, dir.path(), TOLA_META_LABEL);
        assert!(result.is_err(), "Should fail for nonexistent file");
    }

    #[test]
    fn test_compile_with_imports() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        // Create a template file
        let template_dir = dir.path().join("templates");
        fs::create_dir_all(&template_dir).unwrap();
        fs::write(template_dir.join("header.typ"), "#let header = \"My Site\"").unwrap();

        // Create main file that imports template
        let main_file = content_dir.join("main.typ");
        fs::write(
            &main_file,
            "#import \"/templates/header.typ\": header\n= #header\n",
        )
        .unwrap();

        let result = compile_meta(&main_file, dir.path(), TOLA_META_LABEL);
        assert!(result.is_ok(), "Should compile with imports: {:?}", result);
    }

    #[test]
    fn test_multiple_compilations_share_resources() {
        let (dir, file_path) = create_test_project();

        // Multiple compilations should reuse global resources
        for _ in 0..3 {
            let result = compile_meta(&file_path, dir.path(), TOLA_META_LABEL);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_compile_error_shows_formatted_message() {
        super::disable_colors();

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("error.typ");

        // Write a file with a syntax error
        fs::write(&file_path, "#let x = \n= Hello").unwrap();

        let result = compile_meta(&file_path, dir.path(), TOLA_META_LABEL);
        assert!(result.is_err(), "Should fail for syntax error");

        let err_msg = result.unwrap_err().to_string();
        // Error message should contain formatted location info, not raw Debug output
        assert!(
            err_msg.contains("error:"),
            "Error should have 'error:' prefix: {}",
            err_msg
        );
        assert!(
            !err_msg.contains("SourceDiagnostic {"),
            "Error should not contain raw Debug output: {}",
            err_msg
        );
    }

    #[test]
    fn test_compile_error_shows_line_numbers() {
        super::disable_colors();

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("error.typ");

        // Write a file with an undefined variable error
        fs::write(&file_path, "= Title\n\n#undefined_var").unwrap();

        let result = compile_meta(&file_path, dir.path(), TOLA_META_LABEL);
        assert!(result.is_err(), "Should fail for undefined variable");

        let err_msg = result.unwrap_err().to_string();
        // Should contain file and line information
        assert!(
            err_msg.contains("error.typ"),
            "Error should contain filename: {}",
            err_msg
        );
    }

    #[test]
    fn test_query_meta_basic() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("post.typ");

        // Write a file with metadata
        fs::write(
            &file_path,
            r#"#metadata((
  title: "Test Post",
  date: "2024-01-01",
  author: "Test Author",
)) <tola-meta>

= Hello World
"#,
        )
        .unwrap();

        let result = query_meta(&file_path, dir.path(), TOLA_META_LABEL);
        assert!(result.is_ok(), "Query should succeed: {:?}", result);

        // The result should be a dictionary containing our metadata
        let value = result.unwrap();

        // Serialize to JSON to check contents
        let json: serde_json::Value = serde_json::to_value(&value).unwrap();
        assert_eq!(json.get("title").and_then(|v| v.as_str()), Some("Test Post"));
        assert_eq!(json.get("date").and_then(|v| v.as_str()), Some("2024-01-01"));
        assert_eq!(json.get("author").and_then(|v| v.as_str()), Some("Test Author"));
    }

    #[test]
    fn test_query_meta_not_found() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("post.typ");

        // Write a file without the expected label
        fs::write(&file_path, "= Hello World").unwrap();

        let result = query_meta(&file_path, dir.path(), "nonexistent-label");
        assert!(result.is_err(), "Should fail for missing label");
    }
}
