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
//! - [`font`] - Global shared font management with `OnceLock`
//! - [`package`] - Global shared package storage with `LazyLock`
//! - [`library`] - Global shared Typst standard library with `LazyLock`
//! - [`file`] - **Global** file cache with fingerprint-based invalidation
//! - [`world`] - `SystemWorld` implementation of the `World` trait
//! - [`diagnostic`] - Human-readable diagnostic formatting with filtering
//!
//! # Performance Optimizations
//!
//! 1. **Global Shared Fonts** - Font search is expensive (~100ms+). We do it once
//!    at startup and share across all compilations via `OnceLock`.
//!
//! 2. **Global Shared PackageStorage** - Package downloads and caching are shared
//!    to avoid redundant network requests.
//!
//! 3. **Global Shared Library** - Typst's standard library is created once with
//!    HTML feature enabled.
//!
//! 4. **Global File Cache** - Template files, common imports, etc. are cached
//!    globally and reused across ALL file compilations. Only changed files
//!    are re-read (fingerprint-based invalidation).
//!
//! # Usage Example
//!
//! ```ignore
//! use std::path::Path;
//! use tola::typst_lib;
//!
//! // Pre-warm at startup (optional but recommended)
//! typst_lib::warmup_with_root(Path::new("/project/root"));
//!
//! // Compile files - template.typ is cached after first use!
//! let html1 = typst_lib::compile_to_html(
//!     Path::new("/project/content/page1.typ"),
//!     Path::new("/project"),
//! )?;
//! let html2 = typst_lib::compile_to_html(
//!     Path::new("/project/content/page2.typ"),  // Reuses cached template!
//!     Path::new("/project"),
//! )?;
//! ```

mod diagnostic;
mod file;
mod font;
mod library;
mod package;
mod world;

use std::path::Path;

use typst::foundations::{Label, Selector, Value};
use typst::introspection::MetadataElem;
use typst::utils::PicoStr;
use typst::Document;

pub use world::SystemWorld;

/// Compilation result containing HTML output and optional metadata.
///
/// This struct allows a single compilation to produce both the HTML output
/// and any metadata (from `<tola-meta>` labels), avoiding duplicate compilations.
#[derive(Debug)]
pub struct CompileResult {
    /// The compiled HTML content as bytes.
    pub html: Vec<u8>,
    /// Optional metadata value from `<tola-meta>` label.
    /// None if the label doesn't exist in the document.
    pub metadata: Option<serde_json::Value>,
}

/// Test-only mutex to serialize typst compilations.
///
/// Typst's comemo caching can race in parallel test execution.
/// In production, rayon parallel compilation works fine.
#[cfg(test)]
static COMPILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pre-warm global resources (fonts, library, package storage).
///
/// Call this once at startup to avoid lazy initialization during compilation.
/// This moves the ~100ms font loading to a predictable point.
/// Pass the project root to include custom fonts from the project directory.
pub fn warmup_with_root(root: &Path) {
    let _ = font::get_fonts(Some(root));
    let _ = &*library::GLOBAL_LIBRARY;
    let _ = &*package::GLOBAL_PACKAGE_STORAGE;
    let _ = &*file::GLOBAL_FILE_CACHE;
}

/// Compile a Typst file to HTML string.
///
/// This is the main entry point. It creates a lightweight `SystemWorld` that
/// references globally shared fonts/packages/library/file-cache.
///
/// # Arguments
///
/// * `path` - Path to the `.typ` file to compile
/// * `root` - Project root directory for resolving imports
///
/// # Returns
///
/// The compiled HTML as a string, or an error if compilation fails.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read
/// - Typst compilation fails (syntax errors, missing imports, etc.)
/// - HTML export fails
///
/// # Diagnostics
///
/// When compilation fails, the error message includes human-readable diagnostic
/// information with file paths, line numbers, and hints. Known HTML export
/// development warnings are automatically filtered out.
#[allow(dead_code)] // Used by tests
pub fn compile_to_html(path: &Path, root: &Path) -> anyhow::Result<String> {
    // Serialize compilations in tests to avoid comemo race conditions
    #[cfg(test)]
    let _guard = COMPILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Reset access flags to enable fingerprint checking for file changes
    file::reset_access_flags();

    let world = SystemWorld::new(path, root)?;
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

    // Export to HTML
    typst_html::html(&document)
        .map_err(|e| anyhow::anyhow!("HTML export failed: {e:?}"))
}

/// Compile a Typst file and extract metadata in a single compilation pass.
///
/// This is the **recommended** entry point for building sites, as it avoids
/// compiling the same file twice (once for HTML, once for metadata query).
///
/// # Arguments
///
/// * `path` - Path to the `.typ` file to compile
/// * `root` - Project root directory for resolving imports
/// * `label_name` - The label to query for metadata (e.g., "tola-meta")
///
/// # Returns
///
/// A `CompileResult` containing:
/// - `html`: The compiled HTML as bytes
/// - `metadata`: Optional metadata value (None if label not found)
pub fn compile_with_metadata(
    path: &Path,
    root: &Path,
    label_name: &str,
) -> anyhow::Result<CompileResult> {
    // Serialize compilations in tests to avoid comemo race conditions
    #[cfg(test)]
    let _guard = COMPILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Reset access flags to enable fingerprint checking for file changes
    file::reset_access_flags();

    let world = SystemWorld::new(path, root)?;
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

    // Export to HTML
    let html = typst_html::html(&document)
        .map_err(|e| anyhow::anyhow!("HTML export failed: {e:?}"))?
        .into_bytes();

    // Try to extract metadata (optional - don't fail if not found)
    let metadata = extract_metadata(&document, label_name);

    Ok(CompileResult { html, metadata })
}

/// Extract metadata from a compiled document by label name.
fn extract_metadata(document: &typst_html::HtmlDocument, label_name: &str) -> Option<serde_json::Value> {
    let label = Label::new(PicoStr::intern(label_name))?;
    let selector = Selector::Label(label);

    let introspector = document.introspector();
    let elem = introspector.query_unique(&selector).ok()?;

    elem.to_packed::<MetadataElem>()
        .and_then(|meta| serde_json::to_value(&meta.value).ok())
}

/// Query metadata from a Typst file by label name.
///
/// This is equivalent to `typst query <file> "<label>" --field value --one`.
/// It compiles the document and queries for a metadata element with the given label,
/// returning its value as a JSON-serializable Value.
///
/// # Arguments
///
/// * `path` - Path to the `.typ` file to query
/// * `root` - Project root directory for resolving imports
/// * `label_name` - The label to query for (e.g., "tola-meta" for `<tola-meta>`)
///
/// # Returns
///
/// The metadata value if found, or an error if compilation fails or label not found.
#[allow(dead_code)] // Used by tests
pub fn query_metadata(path: &Path, root: &Path, label_name: &str) -> anyhow::Result<Value> {
    // Serialize compilations in tests to avoid comemo race conditions
    #[cfg(test)]
    let _guard = COMPILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Reset access flags to enable fingerprint checking for file changes
    file::reset_access_flags();

    let world = SystemWorld::new(path, root)?;
    let result = typst::compile::<typst_html::HtmlDocument>(&world);

    // Extract document (ignore warnings for query - they were shown during build)
    let document = result.output.map_err(|errors| {
        let all_diags: Vec<_> = errors.iter().cloned().collect();
        let formatted = diagnostic::format_diagnostics(&world, &all_diags);
        anyhow::anyhow!("Typst compilation failed:\n{formatted}")
    })?;

    // Create label selector
    let label = Label::new(PicoStr::intern(label_name))
        .ok_or_else(|| anyhow::anyhow!("Invalid label name: {label_name}"))?;
    let selector = Selector::Label(label);

    // Query the introspector
    let introspector = document.introspector();
    let elem = introspector
        .query_unique(&selector)
        .map_err(|e| anyhow::anyhow!("Query failed: {e}"))?;

    // Extract value from metadata element
    elem.to_packed::<MetadataElem>()
        .map(|meta| meta.value.clone())
        .ok_or_else(|| anyhow::anyhow!("Element is not a metadata element"))
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let result = compile_to_html(&file_path, dir.path());
        assert!(result.is_ok(), "Compilation should succeed");

        let html = result.unwrap();
        assert!(html.contains("Hello World"), "HTML should contain heading");
    }

    #[test]
    fn test_compile_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let fake_path = dir.path().join("nonexistent.typ");

        let result = compile_to_html(&fake_path, dir.path());
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

        let result = compile_to_html(&main_file, dir.path());
        assert!(result.is_ok(), "Should compile with imports: {:?}", result);
    }

    #[test]
    fn test_multiple_compilations_share_resources() {
        let (dir, file_path) = create_test_project();

        // Multiple compilations should reuse global resources
        for _ in 0..3 {
            let result = compile_to_html(&file_path, dir.path());
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_compile_error_shows_formatted_message() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("error.typ");

        // Write a file with a syntax error
        fs::write(&file_path, "#let x = \n= Hello").unwrap();

        let result = compile_to_html(&file_path, dir.path());
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
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("error.typ");

        // Write a file with an undefined variable error
        fs::write(&file_path, "= Title\n\n#undefined_var").unwrap();

        let result = compile_to_html(&file_path, dir.path());
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
    fn test_query_metadata_basic() {
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

        let result = query_metadata(&file_path, dir.path(), "tola-meta");
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
    fn test_query_metadata_not_found() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("post.typ");

        // Write a file without the expected label
        fs::write(&file_path, "= Hello World").unwrap();

        let result = query_metadata(&file_path, dir.path(), "nonexistent-label");
        assert!(result.is_err(), "Should fail for missing label");
    }
}
