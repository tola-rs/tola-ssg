//! Compilation pipeline for hot-reload single-file compiles.
//!
//! `compile_page` compiles and writes page output, and also handles state cleanup
//! for transitions such as "published page -> draft".

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
        /// Compilation warnings (for persistence)
        warnings: Vec<String>,
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
/// This function:
/// - Routes by file extension
/// - Calls the existing `process_page` with Development driver for .typ files
/// - Returns a unified outcome type
/// - Applies draft-transition cleanup when a page becomes non-visible
pub fn compile_page(path: &Path, config: &SiteConfig) -> CompileOutcome {
    let ext = path.extension().and_then(|e| e.to_str());

    match ext {
        Some(e) if ContentKind::from_extension(e).is_some() => {
            // Page pipeline requires sources under content dir.
            // Guard here to avoid passing deps/templates into CompiledPage::from_paths.
            if !path.starts_with(&config.build.content) {
                return CompileOutcome::Skipped;
            }
            compile_content_file(path, config)
        }
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
                // Convert warnings to strings for persistence
                let warnings: Vec<String> =
                    page_result.warnings.iter().map(|w| w.to_string()).collect();

                CompileOutcome::Vdom {
                    path: path.to_path_buf(),
                    url_path: permalink,
                    vdom: Box::new(vdom),
                    warnings,
                }
            } else {
                CompileOutcome::Skipped
            }
        }
        Ok(None) => {
            if cleanup_draft_state(path, config) {
                crate::debug!("watch"; "page became draft: {}", path.display());
            }
            CompileOutcome::Skipped
        }
        Err(e) => CompileOutcome::Error {
            path: path.to_path_buf(),
            url_path: None,
            error: e.to_string(),
        },
    }
}

/// Clean runtime/global state for pages that are now drafts.
///
/// Returns true when a previously visible page existed and was removed.
fn cleanup_draft_state(path: &Path, config: &SiteConfig) -> bool {
    let normalized = crate::utils::path::normalize_path(path);
    let has_alt = normalized.as_path() != path;

    let old_url = crate::address::GLOBAL_ADDRESS_SPACE
        .read()
        .url_for_source(path)
        .cloned()
        .or_else(|| crate::page::STORED_PAGES.get_permalink_by_source(path));
    let old_url = if old_url.is_none() && has_alt {
        crate::address::GLOBAL_ADDRESS_SPACE
            .read()
            .url_for_source(&normalized)
            .cloned()
            .or_else(|| crate::page::STORED_PAGES.get_permalink_by_source(&normalized))
    } else {
        old_url
    };

    // Remove stale runtime state regardless of whether the page had a URL mapping.
    {
        let mut space = crate::address::GLOBAL_ADDRESS_SPACE.write();
        space.remove_by_source(path);
        if has_alt {
            space.remove_by_source(&normalized);
        }
    }
    crate::page::STORED_PAGES.remove_by_source(path);
    if has_alt {
        crate::page::STORED_PAGES.remove_by_source(&normalized);
    }
    crate::compiler::dependency::remove_content(path);
    if has_alt {
        crate::compiler::dependency::remove_content(&normalized);
    }

    let Some(old_url) = old_url else {
        return false;
    };

    // Remove cached VDOM and link-graph edges for this page.
    crate::compiler::page::BUILD_CACHE.remove(&tola_vdom::CacheKey::new(old_url.as_str()));
    crate::page::PAGE_LINKS.record(&old_url, vec![]);

    cleanup_output_file(config, &old_url);
    true
}

fn cleanup_output_file(config: &SiteConfig, url: &UrlPath) {
    let output_dir = config.paths().output_dir();
    let rel_path = url.as_str().trim_matches('/');
    let output_file = if rel_path.is_empty() {
        output_dir.join("index.html")
    } else {
        output_dir.join(rel_path).join("index.html")
    };

    if !output_file.exists() {
        return;
    }

    if let Err(e) = std::fs::remove_file(&output_file) {
        crate::debug!("watch"; "failed to remove {}: {}", output_file.display(), e);
        return;
    }

    if let Some(parent) = output_file.parent()
        && parent != output_dir
        && parent.is_dir()
        && std::fs::read_dir(parent)
            .map(|mut e| e.next().is_none())
            .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn reset_global_state() {
        crate::compiler::page::BUILD_CACHE.clear();
        crate::compiler::dependency::clear_graph();
        crate::page::PAGE_LINKS.clear();
        crate::page::STORED_PAGES.clear();
        crate::address::GLOBAL_ADDRESS_SPACE.write().clear();
    }

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

    #[test]
    fn test_compile_page_skips_typ_outside_content_dir() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let templates_dir = dir.path().join("templates");
        fs::create_dir_all(&templates_dir).unwrap();

        let template = templates_dir.join("post.typ");
        fs::write(&template, "= Template").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        let outcome = compile_page(&template, &config);
        assert!(matches!(outcome, CompileOutcome::Skipped));
    }

    #[test]
    fn test_draft_toggle_false_then_true_cleans_runtime_state() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let page = content_dir.join("post.md");
        let output_file = output_dir.join("post").join("index.html");

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir.clone();
        config.build.output = output_dir.clone();

        reset_global_state();

        // Start as draft (no previously published state): should be skipped.
        fs::write(&page, "---\ntitle: Post\ndraft: true\n---\n\n# Post\n").unwrap();
        let draft_outcome = compile_page(&page, &config);
        assert!(matches!(draft_outcome, CompileOutcome::Skipped));
        assert!(
            crate::page::STORED_PAGES
                .get_permalink_by_source(&page)
                .is_none()
        );
        assert!(!output_file.exists());

        // Simulate previously published state for this source.
        let route = crate::page::CompiledPage::from_paths(&page, &config)
            .unwrap()
            .route;
        crate::page::STORED_PAGES.insert_source_mapping(page.clone(), route.permalink.clone());
        crate::page::STORED_PAGES.insert_page(
            route.permalink.clone(),
            crate::page::PageMeta {
                title: Some("Post".to_string()),
                draft: false,
                ..Default::default()
            },
        );
        crate::address::GLOBAL_ADDRESS_SPACE
            .write()
            .register_page(route.clone(), Some("Post".to_string()));
        if let Some(parent) = output_file.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&output_file, "stale html").unwrap();

        let hash_published = crate::page::STORED_PAGES.pages_hash();

        // Compile as draft again; stale published state must be cleaned.
        let back_to_draft = compile_page(&page, &config);
        assert!(
            matches!(back_to_draft, CompileOutcome::Skipped),
            "expected Skipped when removing published page, got: {:?}",
            back_to_draft
        );
        assert!(
            crate::page::STORED_PAGES
                .get_permalink_by_source(&page)
                .is_none()
        );
        assert!(
            crate::address::GLOBAL_ADDRESS_SPACE
                .read()
                .url_for_source(&page)
                .is_none()
        );
        assert!(!output_file.exists());
        assert_ne!(crate::page::STORED_PAGES.pages_hash(), hash_published);

        reset_global_state();
    }
}
