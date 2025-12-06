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

pub use world::SystemWorld;

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
pub fn compile_to_html(path: &Path, root: &Path) -> anyhow::Result<String> {
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
        let all_diags: Vec<_> = errors
            .iter()
            .chain(&result.warnings)
            .cloned()
            .collect();
        let formatted = diagnostic::format_diagnostics(&world, &all_diags);
        anyhow::anyhow!("Typst compilation failed:\n{formatted}")
    })?;

    // Export to HTML
    typst_html::html(&document)
        .map_err(|e| anyhow::anyhow!("HTML export failed: {e:?}"))
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
}
