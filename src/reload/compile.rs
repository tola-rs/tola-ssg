//! Compilation pipeline for hot-reload single-file compiles.
//!
//! `compile_page` compiles and writes page output, and also handles state cleanup
//! for transitions such as "published page -> draft".

use std::path::{Path, PathBuf};

use crate::compiler::family::Indexed;
use crate::compiler::page::{PageStateTicket, process_page, process_page_with_ticket};
use crate::config::SiteConfig;
use crate::core::{BuildMode, ContentKind, UrlPath};
use crate::page::PageRoute;
use tola_vdom::Document;

/// Result of compiling a single file
#[derive(Debug)]
pub enum CompileOutcome {
    /// Successfully compiled to VDOM
    Vdom {
        path: PathBuf,
        route: PageRoute,
        title: Option<String>,
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
    compile_page_inner(path, config, None)
}

pub fn compile_page_with_ticket(
    path: &Path,
    config: &SiteConfig,
    ticket: &PageStateTicket,
) -> CompileOutcome {
    compile_page_inner(path, config, Some(ticket))
}

fn compile_page_inner(
    path: &Path,
    config: &SiteConfig,
    ticket: Option<&PageStateTicket>,
) -> CompileOutcome {
    let ext = path.extension().and_then(|e| e.to_str());

    match ext {
        Some(e) if ContentKind::from_extension(e).is_some() => {
            // Page pipeline requires sources under content dir.
            // Guard here to avoid passing deps/templates into CompiledPage::from_paths.
            if !path.starts_with(&config.build.content) {
                return CompileOutcome::Skipped;
            }
            compile_content_file(path, config, ticket)
        }
        Some("css" | "js" | "html") => CompileOutcome::Reload {
            reason: format!("asset changed: {}", path.display()),
        },
        // Unknown file types are ignored (whitelist approach)
        // This prevents editor temp files from triggering reload
        _ => CompileOutcome::Skipped,
    }
}

/// Compile a startup batch using the same per-file semantics as hot-reload.
///
/// This is used by `serve` cache startup to ensure changed files are written to disk
/// and draft transitions are cleaned up consistently.
///
/// Keep this intentionally low-concurrency. During `serve` startup we want
/// request-driven compiles to stay responsive; a full-core rayon batch here
/// can make the first interactive request feel like it is waiting for a
/// rebuild even though on-demand compilation exists.
pub fn compile_startup_batch(paths: &[PathBuf], config: &SiteConfig) -> Vec<CompileOutcome> {
    let mut outcomes = Vec::with_capacity(paths.len());
    for path in paths {
        outcomes.push(compile_page(path, config));
        crate::compiler::dependency::flush_current_thread_deps();
    }

    outcomes
}

/// Compile a single content file (Typst or Markdown) to VDOM
fn compile_content_file(
    path: &Path,
    config: &SiteConfig,
    ticket: Option<&PageStateTicket>,
) -> CompileOutcome {
    let result = match ticket {
        Some(ticket) => process_page_with_ticket(BuildMode::DEVELOPMENT, path, config, ticket),
        None => process_page(BuildMode::DEVELOPMENT, path, config),
    };

    match result {
        Ok(Some(page_result)) => {
            let permalink = page_result.permalink;
            let route = page_result.page.route.clone();
            let title = page_result
                .page
                .content_meta
                .as_ref()
                .and_then(|meta| meta.title.clone());

            if let Err(e) = crate::compiler::page::write_page_html(&page_result.page) {
                return CompileOutcome::Error {
                    path: path.to_path_buf(),
                    url_path: Some(permalink),
                    error: format!("failed to write HTML: {}", e),
                };
            }

            if let Some(vdom) = page_result.indexed_vdom {
                // Convert warnings to strings for persistence
                let root = config.get_root();
                let warnings: Vec<String> = page_result
                    .warnings
                    .iter()
                    .map(|w| crate::compiler::page::format_warning_with_prefix(w, root))
                    .collect();

                CompileOutcome::Vdom {
                    path: path.to_path_buf(),
                    route,
                    title,
                    url_path: permalink,
                    vdom: Box::new(vdom),
                    warnings,
                }
            } else {
                CompileOutcome::Skipped
            }
        }
        Ok(None) => {
            let cleaned = match ticket {
                Some(ticket) => ticket
                    .commit(|| cleanup_draft_state(path, config))
                    .unwrap_or(false),
                None => cleanup_draft_state(path, config),
            };
            if cleaned {
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
    cleanup_removed_source_state(path, config).is_some()
}

/// Clean all runtime state for a removed or hidden source file.
///
/// Returns the previously visible URL if one was known.
pub fn cleanup_removed_source_state(path: &Path, config: &SiteConfig) -> Option<UrlPath> {
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
    crate::compiler::scheduler::SCHEDULER.invalidate(path);
    if has_alt {
        crate::compiler::scheduler::SCHEDULER.invalidate(&normalized);
    }

    let Some(old_url) = old_url else {
        return None;
    };

    // Remove cached VDOM and link-graph edges for this page.
    crate::compiler::page::BUILD_CACHE.remove(&tola_vdom::CacheKey::new(old_url.as_str()));
    crate::page::PAGE_LINKS.record(&old_url, vec![]);

    cleanup_output_file(config, &old_url);
    Some(old_url)
}

fn cleanup_output_file(config: &SiteConfig, url: &UrlPath) {
    let output_dir = config.paths().output_dir();
    let output_file = url.output_html_path(&output_dir);

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

/// Remove generated HTML output for a URL if it exists.
pub fn cleanup_output_for_url(config: &SiteConfig, url: &UrlPath) {
    cleanup_output_file(config, url);
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
        let output_file = UrlPath::from_page("/post/").output_html_path(&output_dir);

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
        assert!(
            crate::page::PAGE_LINKS
                .links_to(&route.permalink)
                .is_empty()
        );
        assert_ne!(crate::page::STORED_PAGES.pages_hash(), hash_published);

        reset_global_state();
    }

    #[test]
    fn cleanup_removed_source_state_clears_page_links() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let page = content_dir.join("post.md");
        fs::write(&page, "---\ntitle: Post\n---\n\n# Post\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir.clone();
        config.build.output = output_dir.clone();

        reset_global_state();

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
        crate::page::PAGE_LINKS.record(&route.permalink, vec![UrlPath::from_page("/target/")]);

        let removed = cleanup_removed_source_state(&page, &config);

        assert_eq!(removed, Some(route.permalink.clone()));
        assert!(
            crate::page::PAGE_LINKS
                .links_to(&route.permalink)
                .is_empty()
        );
        reset_global_state();
    }

    #[test]
    fn test_compile_page_with_stale_ticket_does_not_cleanup_draft_state() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let page = content_dir.join("post.md");
        fs::write(&page, "---\ntitle: Post\ndraft: true\n---\n\n# Post\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;
        config.build.output = output_dir;

        reset_global_state();

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

        let epoch = crate::compiler::page::PageStateEpoch::new();
        let ticket = epoch.ticket();
        epoch.advance();

        let outcome = compile_page_with_ticket(&page, &config, &ticket);

        assert!(matches!(outcome, CompileOutcome::Skipped));
        assert_eq!(
            crate::page::STORED_PAGES.get_permalink_by_source(&page),
            Some(route.permalink.clone())
        );
        assert!(
            crate::address::GLOBAL_ADDRESS_SPACE
                .read()
                .url_for_source(&crate::utils::path::normalize_path(&page))
                .is_some()
        );

        reset_global_state();
    }
}
