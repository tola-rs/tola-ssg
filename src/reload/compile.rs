//! Compilation pipeline for hot-reload single-file compiles.
//!
//! `compile_page` compiles and writes page output, and also handles state cleanup
//! for transitions such as "published page -> draft".

use std::path::{Path, PathBuf};

use crate::address::{PermalinkUpdate, SiteIndex};
use crate::compiler::family::Indexed;
use crate::compiler::page::{PageStateTicket, PreparedPage, commit_page_state_parts, prepare_page};
use crate::config::SiteConfig;
use crate::core::{BuildMode, ContentKind, UrlPath};
use crate::page::PageState;
use tola_vdom::Document;

/// Result of compiling a single file
#[derive(Debug)]
pub enum CompileOutcome {
    /// Successfully compiled to VDOM
    Vdom {
        path: PathBuf,
        url_path: UrlPath,
        vdom: Box<Document<Indexed>>,
        permalink_change: Option<PermalinkUpdate>,
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
/// - Prepares page output with the Development driver
/// - Returns a unified outcome type
/// - Applies draft-transition cleanup when a page becomes non-visible
pub fn compile_page(path: &Path, config: &SiteConfig, state: &SiteIndex) -> CompileOutcome {
    compile_page_inner(path, config, state, None)
}

pub fn compile_page_with_ticket(
    path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
    ticket: &PageStateTicket,
) -> CompileOutcome {
    compile_page_inner(path, config, state, Some(ticket))
}

fn compile_page_inner(
    path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
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
            compile_content_file(path, config, state, ticket)
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
pub fn compile_startup_batch(
    paths: &[PathBuf],
    config: &SiteConfig,
    state: &SiteIndex,
) -> Vec<CompileOutcome> {
    let mut outcomes = Vec::with_capacity(paths.len());
    for path in paths {
        outcomes.push(compile_page(path, config, state));
        crate::compiler::dependency::flush_current_thread_deps();
    }

    outcomes
}

/// Compile a single content file (Typst or Markdown) to VDOM
fn compile_content_file(
    path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
    ticket: Option<&PageStateTicket>,
) -> CompileOutcome {
    let result = prepare_page(BuildMode::DEVELOPMENT, path, config, state);

    match result {
        Ok(Some(prepared)) => finish_prepared_page(path, config, state, ticket, prepared),
        Ok(None) => {
            let cleaned = match ticket {
                Some(ticket) => ticket
                    .commit(|| cleanup_draft_state(path, config, state))
                    .unwrap_or(false),
                None => cleanup_draft_state(path, config, state),
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

enum CommitPreparedError {
    Conflict(PermalinkUpdate),
    Write(String),
}

fn finish_prepared_page(
    path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
    ticket: Option<&PageStateTicket>,
    prepared: PreparedPage,
) -> CompileOutcome {
    let commit_result = match ticket {
        Some(ticket) => {
            match ticket.commit(|| write_and_commit_prepared(path, config, state, &prepared)) {
                Some(result) => result,
                None => return CompileOutcome::Skipped,
            }
        }
        None => write_and_commit_prepared(path, config, state, &prepared),
    };

    let permalink = prepared.result.permalink;
    let permalink_change = match commit_result {
        Ok(update) => update,
        Err(CommitPreparedError::Conflict(update)) => Some(update),
        Err(CommitPreparedError::Write(error)) => {
            return CompileOutcome::Error {
                path: path.to_path_buf(),
                url_path: Some(permalink),
                error,
            };
        }
    };

    let Some(vdom) = prepared.result.indexed_vdom else {
        return CompileOutcome::Skipped;
    };

    let root = config.get_root();
    let warnings: Vec<String> = prepared
        .result
        .warnings
        .iter()
        .map(|w| crate::compiler::page::format_warning_with_prefix(w, root))
        .collect();

    CompileOutcome::Vdom {
        path: path.to_path_buf(),
        url_path: permalink,
        vdom: Box::new(vdom),
        permalink_change,
        warnings,
    }
}

fn write_and_commit_prepared(
    path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
    prepared: &PreparedPage,
) -> Result<Option<PermalinkUpdate>, CommitPreparedError> {
    let route = prepared.result.page.route.clone();
    let title = prepared
        .result
        .page
        .content_meta
        .as_ref()
        .and_then(|meta| meta.title.clone());

    state.edit(|store, address| {
        let update = address.check_page_update(&route);
        if matches!(update, PermalinkUpdate::Conflict { .. }) {
            return Err(CommitPreparedError::Conflict(update));
        }

        crate::compiler::page::write_page_html(&prepared.result.page)
            .map_err(|e| CommitPreparedError::Write(format!("failed to write HTML: {}", e)))?;

        let applied = address.update_page_checked(route, title);
        commit_page_state_parts(
            store,
            address,
            path,
            &prepared.result.page,
            &prepared.scan_data,
            config,
        );

        Ok(match applied {
            PermalinkUpdate::Unchanged => None,
            update => Some(update),
        })
    })
}

/// Clean runtime/global state for pages that are now drafts.
///
/// Returns true when a previously visible page existed and was removed.
fn cleanup_draft_state(path: &Path, config: &SiteConfig, state: &SiteIndex) -> bool {
    cleanup_removed_source_state(path, config, state).is_some()
}

/// Clean all runtime state for a removed or hidden source file.
///
/// Returns the previously visible URL if one was known.
pub fn cleanup_removed_source_state(
    path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
) -> Option<UrlPath> {
    let normalized = crate::utils::path::normalize_path(path);
    let has_alt = normalized.as_path() != path;

    let old_url = state.read(|pages, address| {
        address
            .url_for_source(path)
            .cloned()
            .or_else(|| pages.get_permalink_by_source(path))
    });
    let old_url = if old_url.is_none() && has_alt {
        state.read(|pages, address| {
            address
                .url_for_source(&normalized)
                .cloned()
                .or_else(|| pages.get_permalink_by_source(&normalized))
        })
    } else {
        old_url
    };

    // Remove stale runtime state regardless of whether the page had a URL mapping.
    state.edit(|pages, address| {
        address.remove_by_source(path);
        if has_alt {
            address.remove_by_source(&normalized);
        }
        pages.remove_by_source(path);
        if has_alt {
            pages.remove_by_source(&normalized);
        }
        if let Some(old_url) = &old_url {
            PageState::new(pages).clear_links(old_url);
        }
    });

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

    fn reset_state(state: &SiteIndex) {
        crate::compiler::page::BUILD_CACHE.clear();
        crate::compiler::dependency::clear_graph();
        state.clear();
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
        let state = SiteIndex::new();

        let outcome = compile_page(&template, &config, &state);
        assert!(matches!(outcome, CompileOutcome::Skipped));
    }

    #[test]
    fn permalink_conflict_does_not_commit_page_state_or_output() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let existing = content_dir.join("existing.md");
        let incoming = content_dir.join("incoming.md");
        fs::write(&existing, "+++\ntitle = \"Existing\"\n+++\n\n# Existing\n").unwrap();
        fs::write(
            &incoming,
            "+++\ntitle = \"Incoming\"\npermalink = \"/taken/\"\n+++\n\n# Incoming\n",
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;
        config.build.output = output_dir.clone();
        let state = SiteIndex::new();
        let store = state.pages();

        reset_state(&state);

        let existing_meta = crate::page::PageMeta {
            title: Some("Existing".to_string()),
            permalink: Some("/taken/".to_string()),
            ..Default::default()
        };
        let existing_page = crate::page::CompiledPage::from_paths_with_meta(
            &existing,
            &config,
            Some(existing_meta),
        )
        .unwrap();
        store.insert_source_mapping(existing.clone(), existing_page.route.permalink.clone());
        store.insert_page(
            existing_page.route.permalink.clone(),
            existing_page.content_meta.clone().unwrap_or_default(),
        );
        state.edit(|_, address| {
            address.register_page(existing_page.route.clone(), Some("Existing".to_string()));
        });

        let outcome = compile_page(&incoming, &config, &state);

        match outcome {
            CompileOutcome::Vdom {
                permalink_change:
                    Some(PermalinkUpdate::Conflict {
                        url,
                        existing_source,
                    }),
                ..
            } => {
                assert_eq!(url, UrlPath::from_page("/taken/"));
                assert_eq!(
                    existing_source,
                    crate::utils::path::normalize_path(&existing)
                );
            }
            other => panic!("expected permalink conflict, got: {:?}", other),
        }

        assert!(store.get_permalink_by_source(&incoming).is_none());
        assert!(state.read(|_, address| address.url_for_source(&incoming).is_none()));
        assert!(
            !store
                .get_pages_with_drafts()
                .iter()
                .any(|page| page.meta.title.as_deref() == Some("Incoming"))
        );
        assert!(
            !UrlPath::from_page("/taken/")
                .output_html_path(&output_dir)
                .exists()
        );

        reset_state(&state);
    }

    #[test]
    fn write_failure_does_not_commit_page_state() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let page = content_dir.join("post.md");
        fs::write(&page, "+++\ntitle = \"Post\"\n+++\n\n# Post\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;
        config.build.output = output_dir.clone();
        let state = SiteIndex::new();
        let store = state.pages();

        reset_state(&state);

        let output_file = UrlPath::from_page("/post/").output_html_path(&output_dir);
        fs::create_dir_all(&output_file).unwrap();

        let outcome = compile_page(&page, &config, &state);

        match outcome {
            CompileOutcome::Error {
                url_path: Some(url),
                error,
                ..
            } => {
                assert_eq!(url, UrlPath::from_page("/post/"));
                assert!(error.contains("failed to write HTML"));
            }
            other => panic!("expected write error, got: {:?}", other),
        }

        assert!(store.get_permalink_by_source(&page).is_none());
        assert!(store.get_pages_with_drafts().is_empty());
        assert!(state.read(|_, address| address.url_for_source(&page).is_none()));

        reset_state(&state);
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
        let state = SiteIndex::new();
        let store = state.pages();

        reset_state(&state);

        // Start as draft (no previously published state): should be skipped.
        fs::write(&page, "---\ntitle: Post\ndraft: true\n---\n\n# Post\n").unwrap();
        let draft_outcome = compile_page(&page, &config, &state);
        assert!(matches!(draft_outcome, CompileOutcome::Skipped));
        assert!(store.get_permalink_by_source(&page).is_none());
        assert!(!output_file.exists());

        // Simulate previously published state for this source.
        let route = crate::page::CompiledPage::from_paths(&page, &config)
            .unwrap()
            .route;
        store.insert_source_mapping(page.clone(), route.permalink.clone());
        store.insert_page(
            route.permalink.clone(),
            crate::page::PageMeta {
                title: Some("Post".to_string()),
                draft: false,
                ..Default::default()
            },
        );
        state.edit(|_, address| address.register_page(route.clone(), Some("Post".to_string())));
        if let Some(parent) = output_file.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&output_file, "stale html").unwrap();

        let hash_published = store.pages_hash();

        // Compile as draft again; stale published state must be cleaned.
        let back_to_draft = compile_page(&page, &config, &state);
        assert!(
            matches!(back_to_draft, CompileOutcome::Skipped),
            "expected Skipped when removing published page, got: {:?}",
            back_to_draft
        );
        assert!(store.get_permalink_by_source(&page).is_none());
        assert!(state.read(|_, address| address.url_for_source(&page).is_none()));
        assert!(!output_file.exists());
        assert!(PageState::new(store).links_to(&route.permalink).is_empty());
        assert_ne!(store.pages_hash(), hash_published);

        reset_state(&state);
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
        let state = SiteIndex::new();
        let store = state.pages();

        reset_state(&state);

        let route = crate::page::CompiledPage::from_paths(&page, &config)
            .unwrap()
            .route;
        store.insert_source_mapping(page.clone(), route.permalink.clone());
        store.insert_page(
            route.permalink.clone(),
            crate::page::PageMeta {
                title: Some("Post".to_string()),
                draft: false,
                ..Default::default()
            },
        );
        state.edit(|_, address| address.register_page(route.clone(), Some("Post".to_string())));
        PageState::new(store).record_links(&route.permalink, vec![UrlPath::from_page("/target/")]);

        let removed = cleanup_removed_source_state(&page, &config, &state);

        assert_eq!(removed, Some(route.permalink.clone()));
        assert!(PageState::new(store).links_to(&route.permalink).is_empty());
        reset_state(&state);
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
        let state = SiteIndex::new();
        let store = state.pages();

        reset_state(&state);

        let route = crate::page::CompiledPage::from_paths(&page, &config)
            .unwrap()
            .route;
        store.insert_source_mapping(page.clone(), route.permalink.clone());
        store.insert_page(
            route.permalink.clone(),
            crate::page::PageMeta {
                title: Some("Post".to_string()),
                draft: false,
                ..Default::default()
            },
        );
        state.edit(|_, address| address.register_page(route.clone(), Some("Post".to_string())));

        let epoch = crate::compiler::page::PageStateEpoch::new();
        let ticket = epoch.ticket();
        epoch.advance();

        let outcome = compile_page_with_ticket(&page, &config, &state, &ticket);

        assert!(matches!(outcome, CompileOutcome::Skipped));
        assert_eq!(
            store.get_permalink_by_source(&page),
            Some(route.permalink.clone())
        );
        assert!(state.read(|_, address| {
            address
                .url_for_source(&crate::utils::path::normalize_path(&page))
                .is_some()
        }));

        reset_state(&state);
    }
}
