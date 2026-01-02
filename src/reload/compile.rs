//! Compilation Pipeline - Typst to VDOM
//!
//! Pure functions for compiling Typst files to VDOM.
//! No Actor machinery, minimal side effects.
//!
//! # Side Effect Isolation
//!
//! `compile_page` has ONE side effect: writing HTML to disk.
//! This is intentional - we want compilation to be atomic (compile + write).
//! The alternative (returning HTML string) would require callers to handle writes,
//! complicating error handling and atomicity.

use std::path::{Path, PathBuf};

use crate::compiler::family::Indexed;
use crate::compiler::page::process_page;
use crate::config::SiteConfig;
use crate::core::{BuildMode, ContentKind, UrlPath};
use tola_vdom::Document;

/// Result of compiling a single file
#[derive(Debug)]
pub enum CompileOutcome {
    /// Successfully compiled to VDOM
    Vdom {
        path: PathBuf,
        url_path: UrlPath,
        vdom: Box<Document<Indexed>>,
    },
    /// Non-content file changed, needs full reload
    Reload { reason: String },
    /// File skipped (draft, not found, etc.)
    Skipped,
    /// Compilation error
    Error {
        path: PathBuf,
        url_path: Option<UrlPath>,
        error: String,
    },
}

/// Compile a single file to VDOM
///
/// This is a pure function that:
/// - Routes by file extension
/// - Calls the existing `process_page` with Development driver for .typ files
/// - Returns a unified outcome type
pub fn compile_page(path: &Path, config: &SiteConfig) -> CompileOutcome {
    let ext = path.extension().and_then(|e| e.to_str());

    match ext {
        Some(e) if ContentKind::from_extension(e).is_some() => compile_content_file(path, config),
        Some("css" | "js" | "html") => CompileOutcome::Reload {
            reason: format!("asset changed: {}", path.display()),
        },
        // Unknown file types are ignored (whitelist approach)
        // This prevents editor temp files from triggering reload
        _ => CompileOutcome::Skipped,
    }
}

/// Compile a single content file (Typst or Markdown) to VDOM
fn compile_content_file(path: &Path, config: &SiteConfig) -> CompileOutcome {
    match process_page(BuildMode::DEVELOPMENT, path, config) {
        Ok(Some(page_result)) => {
            let permalink = page_result.permalink;

            if let Err(e) = crate::compiler::page::write_page_html(&page_result.page) {
                return CompileOutcome::Error {
                    path: path.to_path_buf(),
                    url_path: Some(permalink),
                    error: format!("failed to write HTML: {}", e),
                };
            }

            if let Some(vdom) = page_result.indexed_vdom {
                CompileOutcome::Vdom {
                    path: path.to_path_buf(),
                    url_path: permalink,
                    vdom: Box::new(vdom),
                }
            } else {
                CompileOutcome::Skipped
            }
        }
        Ok(None) => CompileOutcome::Skipped,
        Err(e) => CompileOutcome::Error {
            path: path.to_path_buf(),
            url_path: None,
            error: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_outcome_variants() {
        let _ = CompileOutcome::Reload {
            reason: "test".to_string(),
        };
        let _ = CompileOutcome::Skipped;
        let _ = CompileOutcome::Error {
            path: PathBuf::from("/test.typ"),
            url_path: None,
            error: "test error".to_string(),
        };
    }
}
