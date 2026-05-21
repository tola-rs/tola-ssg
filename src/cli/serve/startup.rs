use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rustc_hash::{FxHashMap, FxHashSet};
use tola_vdom::CacheKey;

use super::{bind_server, init_serve_build, ready::ServeReady, scan_pages, start_serve_build};
use crate::address::SiteIndex;
use crate::cache::{self, PersistedDiagnostics, PersistedError, RemovedFile};
use crate::compiler::dependency::{self, collect_virtual_dependents};
use crate::compiler::page::{BUILD_CACHE, TypstHost, cache_vdom};
use crate::compiler::scheduler::SCHEDULER;
use crate::config::{self, SiteConfig};
use crate::core::UrlPath;
use crate::page::PageState;
use crate::reload::compile::{self, CompileOutcome};
use crate::{debug, log, logger};

/// Keep cache-startup repair work narrow so request-driven compiles can still
/// win the machine while startup catches up on offline changes.
const STARTUP_COMPILE_BATCH_SIZE: usize = 1;
const STARTUP_IDLE_GRACE: Duration = Duration::from_millis(250);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Start serve with cached build support
pub fn serve_with_cache(config: &SiteConfig) -> Result<()> {
    use crate::core::{set_healthy, set_serving};
    let state = Arc::new(SiteIndex::new());
    let ready = Arc::new(ServeReady::new());

    if config.build.clean
        && let Err(e) = cache::clear_cache_dir(config.get_root())
    {
        debug!("serve"; "failed to clear vdom cache: {}", e);
    }

    let has_cache =
        !config.build.clean && cache::has_cache(config.get_root()) && config.build.output.exists();
    debug!(
        "startup";
        "serve startup path: {}",
        if has_cache { "cache" } else { "full-build" }
    );

    SCHEDULER.start_workers();
    ready.reset_startup();

    let config_handle = config::config_handle();
    let config_arc = config_handle.current();
    let build_success = if has_cache {
        let build_success = startup_with_cache(&config_arc, &state, &ready)?;
        ready.set_startup_done();
        Some(build_success)
    } else {
        progressive_scan(&config_arc, &state, &ready)?;
        ready.set_scan_ready(true);
        None
    };

    let bound_server = bind_server()?;
    set_serving();

    if let Some(build_success) = build_success {
        set_healthy(build_success);
        if build_success {
            config_handle.clear_clean_flag();
        }
    } else {
        set_healthy(true);
        start_serve_build(
            Arc::clone(&config_arc),
            Arc::new(TypstHost::for_config(&config_arc)),
            Arc::clone(&state),
            Arc::clone(&ready),
        );
    }

    bound_server.run(state, ready)
}

fn progressive_scan(config: &SiteConfig, state: &SiteIndex, ready: &ServeReady) -> Result<()> {
    use crate::core::is_shutdown;

    let typst_host = init_serve_build(config).context("serve startup init failed")?;
    ready.set_init_ready();

    if is_shutdown() {
        return Ok(());
    }

    scan_pages(config, &typst_host, state).context("serve startup scan failed")?;

    if is_shutdown() {
        return Ok(());
    }

    Ok(())
}

fn startup_with_cache(config: &SiteConfig, state: &SiteIndex, ready: &ServeReady) -> Result<bool> {
    let typst_host = init_serve_build(config).context("cache startup init failed")?;
    ready.set_init_ready();

    scan_pages(config, &typst_host, state).context("cache startup scan failed")?;
    ready.set_scan_ready(true);

    let root = config.get_root();
    let mut diagnostics = cache::restore_diagnostics(root).unwrap_or_default();
    let mut files_to_compile = FxHashSet::default();
    let mut error_files = 0usize;
    let mut stale_diagnostics = Vec::new();

    for error in diagnostics.errors() {
        let abs_path = crate::utils::path::normalize_path(&config.root_join(&error.path));
        if abs_path.exists() {
            files_to_compile.insert(abs_path);
            error_files += 1;
        } else {
            stale_diagnostics.push(error.path.clone());
        }
    }
    for path in stale_diagnostics {
        diagnostics.clear_for(&path);
    }

    let modified = cache::get_modified_files(root, &config.build.content);

    debug!(
        "startup";
        "offline changes: errors={}, created={}, removed={}, modified={}",
        error_files,
        modified.created.len(),
        modified.removed.len(),
        modified.modified.len()
    );

    cleanup_removed_files(&modified.removed, config, state, &mut diagnostics);

    for path in modified.created {
        files_to_compile.insert(path);
    }
    for path in modified.modified {
        files_to_compile.insert(path);
    }

    let mut compile_targets: Vec<_> = files_to_compile.into_iter().collect();
    compile_targets.sort();

    let pages_hash = state.with_pages(|pages| pages.pages_hash());
    let mut stats = StartupCompileStats::default();
    if !compile_targets.is_empty() {
        stats = compile_startup_batch(
            &compile_targets,
            &modified.cached_urls_by_source,
            config,
            &typst_host,
            state,
            &mut diagnostics,
            ready,
        );
    }

    if state.with_pages(|pages| pages.pages_hash()) != pages_hash {
        let dependents = collect_virtual_dependents();
        if !dependents.is_empty() {
            let virtual_stats = compile_startup_batch(
                &dependents.into_iter().collect::<Vec<_>>(),
                &FxHashMap::default(),
                config,
                &typst_host,
                state,
                &mut diagnostics,
                ready,
            );
            stats.success += virtual_stats.success;
            stats.failed += virtual_stats.failed;
            stats.skipped += virtual_stats.skipped;
        }
    }

    if let Err(e) = cache::persist_diagnostics(&diagnostics, root) {
        debug!("startup"; "failed to persist diagnostics: {}", e);
    }

    if let Some(first_error) = diagnostics.first_error() {
        let summary = format!(
            "errors: {}, warnings: {}",
            diagnostics.error_count(),
            diagnostics.warning_count()
        );
        let detail = format!("first error: {}\n{}", first_error.path, first_error.error);
        logger::WatchStatus::new().error(&summary, &detail);
    }

    debug!(
        "startup";
        "compile result: success={}, failed={}, skipped={}",
        stats.success,
        stats.failed,
        stats.skipped
    );

    if stats.failed == 0 && compile_targets.is_empty() && modified.removed.is_empty() {
        log!("serve"; "using cached build");
    } else if stats.failed == 0 {
        log!("serve"; "using cached build (startup compiled {} files)", stats.success);
    } else {
        log!("serve"; "using cached build (startup compile errors: {})", stats.failed);
    }

    Ok(true)
}

#[derive(Debug, Default)]
struct StartupCompileStats {
    success: usize,
    failed: usize,
    skipped: usize,
}

fn cleanup_removed_files(
    removed: &[RemovedFile],
    config: &SiteConfig,
    state: &SiteIndex,
    diagnostics: &mut PersistedDiagnostics,
) {
    if removed.is_empty() {
        return;
    }

    for item in removed {
        SCHEDULER.invalidate(&item.source_path);
        state.edit(|pages, address| {
            address.remove_by_source(&item.source_path);
            pages.remove_by_source(&item.source_path);
        });
        dependency::remove_content(&item.source_path);
        cleanup_url_artifacts(config, state, &item.url_path);

        let rel = relative_source_path(config, &item.source_path);
        diagnostics.clear_for(&rel);
    }
}

fn cleanup_url_artifacts(config: &SiteConfig, state: &SiteIndex, url: &UrlPath) {
    BUILD_CACHE.remove(&CacheKey::new(url.as_str()));
    state.with_pages(|pages| PageState::new(pages).clear_links(url));
    compile::cleanup_output_for_url(config, url);
}

fn cleanup_cached_url(
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    source_path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
) {
    if let Some(old_url) = cached_urls.get(source_path) {
        cleanup_url_artifacts(config, state, old_url);
    }
}

fn cleanup_cached_url_if_changed(
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    source_path: &Path,
    new_url: &UrlPath,
    config: &SiteConfig,
    state: &SiteIndex,
) {
    if let Some(old_url) = cached_urls.get(source_path)
        && old_url != new_url
    {
        cleanup_url_artifacts(config, state, old_url);
    }
}

fn relative_source_path(config: &SiteConfig, path: &Path) -> String {
    path.strip_prefix(config.get_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

struct StartupContext<'a> {
    cached_urls: &'a FxHashMap<PathBuf, UrlPath>,
    config: &'a SiteConfig,
    state: &'a SiteIndex,
    diagnostics: &'a mut PersistedDiagnostics,
}

fn handle_startup_vdom_outcome(
    path: PathBuf,
    url_path: UrlPath,
    vdom: Box<tola_vdom::Document<crate::compiler::family::Indexed>>,
    warnings: Vec<String>,
    ctx: &mut StartupContext<'_>,
) {
    cleanup_cached_url_if_changed(ctx.cached_urls, &path, &url_path, ctx.config, ctx.state);
    cache_vdom(&url_path, *vdom);

    let rel = relative_source_path(ctx.config, &path);
    ctx.diagnostics.clear_errors_for(&rel);
    ctx.diagnostics.set_warnings(&rel, warnings);
}

fn handle_startup_error_outcome(
    path: PathBuf,
    url_path: Option<UrlPath>,
    error: String,
    config: &SiteConfig,
    diagnostics: &mut PersistedDiagnostics,
) {
    let rel = relative_source_path(config, &path);
    diagnostics.clear_warnings_for(&rel);
    diagnostics.push_error(PersistedError::new(
        rel,
        url_path.unwrap_or_default().to_string(),
        error,
    ));
}

fn handle_startup_skipped_outcome(
    input_path: &Path,
    rel_input: &str,
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    config: &SiteConfig,
    state: &SiteIndex,
    diagnostics: &mut PersistedDiagnostics,
) {
    cleanup_cached_url(cached_urls, input_path, config, state);
    diagnostics.clear_for(rel_input);
}

fn compile_startup_batch(
    paths: &[PathBuf],
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    config: &SiteConfig,
    typst_host: &TypstHost,
    state: &SiteIndex,
    diagnostics: &mut PersistedDiagnostics,
    ready: &ServeReady,
) -> StartupCompileStats {
    let mut stats = StartupCompileStats::default();
    let mut ctx = StartupContext {
        cached_urls,
        config,
        state,
        diagnostics,
    };

    for path_chunk in paths.chunks(STARTUP_COMPILE_BATCH_SIZE) {
        while !ready.request_idle_for(STARTUP_IDLE_GRACE) {
            if crate::core::is_shutdown() {
                return stats;
            }
            std::thread::sleep(STARTUP_POLL_INTERVAL);
        }

        let outcomes = compile::compile_startup_batch(path_chunk, config, typst_host, state);

        for (input_path, outcome) in path_chunk.iter().zip(outcomes.into_iter()) {
            let rel_input = input_path
                .strip_prefix(config.get_root())
                .unwrap_or(input_path)
                .display()
                .to_string();

            match outcome {
                CompileOutcome::Vdom {
                    path,
                    url_path,
                    vdom,
                    warnings,
                    ..
                } => {
                    handle_startup_vdom_outcome(path, url_path, vdom, warnings, &mut ctx);
                    stats.success += 1;
                }
                CompileOutcome::Error {
                    path,
                    url_path,
                    error,
                } => {
                    handle_startup_error_outcome(
                        path,
                        url_path,
                        error,
                        ctx.config,
                        ctx.diagnostics,
                    );
                    stats.failed += 1;
                }
                CompileOutcome::Skipped => {
                    handle_startup_skipped_outcome(
                        input_path,
                        &rel_input,
                        ctx.cached_urls,
                        ctx.config,
                        ctx.state,
                        ctx.diagnostics,
                    );
                    stats.skipped += 1;
                }
                CompileOutcome::Reload { reason } => {
                    debug!("startup"; "startup compile requested reload: {}", reason);
                    ctx.diagnostics.clear_for(&rel_input);
                    stats.skipped += 1;
                }
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freshness;
    use std::fs;
    use tempfile::TempDir;

    fn make_test_config(root: &Path) -> SiteConfig {
        let root = crate::utils::path::normalize_path(root);
        let mut config = SiteConfig::default();
        config.set_root(&root);
        config.config_path = root.join("tola.toml");
        config.build.content = root.join("content");
        config.build.output = root.join("public");
        fs::create_dir_all(&config.build.content).unwrap();
        fs::create_dir_all(&config.build.output).unwrap();
        config
    }

    fn reset_global_state(state: &SiteIndex) {
        BUILD_CACHE.clear();
        state.clear();
        dependency::clear_graph();
        freshness::clear_cache();
    }

    fn typst_host(config: &SiteConfig) -> TypstHost {
        TypstHost::for_config(config)
    }

    fn write_markdown(path: &Path, heading: &str, draft: bool) {
        let draft_line = if draft { "draft: true\n" } else { "" };
        fs::write(
            path,
            format!(
                "---\ntitle: \"{}\"\n{}---\n\n# {}\n",
                heading, draft_line, heading
            ),
        )
        .unwrap();
    }

    fn write_markdown_with_permalink(path: &Path, heading: &str, permalink: &str) {
        fs::write(
            path,
            format!(
                "+++\ntitle = \"{}\"\npermalink = \"{}\"\n+++\n\n# {}\n",
                heading, permalink, heading
            ),
        )
        .unwrap();
    }

    fn output_file_for(config: &SiteConfig, url: &UrlPath) -> PathBuf {
        url.output_html_path(&config.paths().output_dir())
    }

    fn ready() -> ServeReady {
        let ready = ServeReady::new();
        ready.set_init_ready();
        ready.set_scan_ready(true);
        ready
    }

    #[test]
    fn startup_batch_skipped_draft_cleans_cached_output() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let state = SiteIndex::new();
        reset_global_state(&state);

        let source = config.build.content.join("post.md");
        write_markdown(&source, "Draft Post", true);
        let source = crate::utils::path::normalize_path(&source);

        let old_url = UrlPath::from_page("/legacy/");
        let output_file = output_file_for(&config, &old_url);
        fs::create_dir_all(output_file.parent().unwrap()).unwrap();
        fs::write(&output_file, "stale output").unwrap();
        state.with_pages(|pages| {
            PageState::new(pages).record_links(&old_url, vec![UrlPath::from_page("/target/")]);
        });

        let rel = config.root_relative(&source).display().to_string();
        let mut diagnostics = PersistedDiagnostics::new();
        diagnostics.push_error(PersistedError::new(&rel, old_url.to_string(), "old error"));
        diagnostics.set_warnings(&rel, vec!["old warning".to_string()]);

        let mut cached_urls = FxHashMap::default();
        cached_urls.insert(source.clone(), old_url.clone());
        let host = typst_host(&config);
        let ready = ready();

        let stats = compile_startup_batch(
            std::slice::from_ref(&source),
            &cached_urls,
            &config,
            &host,
            &state,
            &mut diagnostics,
            &ready,
        );

        assert_eq!(stats.success, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.skipped, 1);
        assert!(!output_file.exists(), "stale output should be removed");
        assert!(state.with_pages(|pages| PageState::new(pages).links_to(&old_url).is_empty()));
        assert_eq!(diagnostics.error_count(), 0);
        assert_eq!(diagnostics.warning_count(), 0);

        reset_global_state(&state);
    }

    #[test]
    fn startup_batch_success_writes_updated_html() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let state = SiteIndex::new();
        reset_global_state(&state);

        let source = config.build.content.join("post.md");
        write_markdown(&source, "Startup Modified", false);
        let source = crate::utils::path::normalize_path(&source);

        let rel = config.root_relative(&source).display().to_string();
        let mut diagnostics = PersistedDiagnostics::new();
        diagnostics.push_error(PersistedError::new(&rel, "/post/", "old compile error"));
        let host = typst_host(&config);
        let ready = ready();

        let stats = compile_startup_batch(
            std::slice::from_ref(&source),
            &FxHashMap::default(),
            &config,
            &host,
            &state,
            &mut diagnostics,
            &ready,
        );

        let output_file = output_file_for(&config, &UrlPath::from_page("/post/"));
        let html = fs::read_to_string(&output_file).unwrap_or_default();

        assert_eq!(stats.success, 1);
        assert_eq!(stats.failed, 0);
        assert!(output_file.exists());
        assert!(html.contains("Startup Modified"));
        assert_eq!(diagnostics.error_count(), 0);

        reset_global_state(&state);
    }

    #[test]
    fn startup_batch_multiple_paths_accumulates_results_across_chunks() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let state = SiteIndex::new();
        reset_global_state(&state);

        let first = config.build.content.join("first.md");
        let second = config.build.content.join("second.md");
        write_markdown(&first, "First Startup Page", false);
        write_markdown(&second, "Second Startup Page", false);
        let first = crate::utils::path::normalize_path(&first);
        let second = crate::utils::path::normalize_path(&second);
        let host = typst_host(&config);
        let ready = ready();

        let stats = compile_startup_batch(
            &[first.clone(), second.clone()],
            &FxHashMap::default(),
            &config,
            &host,
            &state,
            &mut PersistedDiagnostics::new(),
            &ready,
        );

        let first_html = output_file_for(&config, &UrlPath::from_page("/first/"));
        let second_html = output_file_for(&config, &UrlPath::from_page("/second/"));

        assert_eq!(stats.success, 2);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.skipped, 0);
        assert!(first_html.exists());
        assert!(second_html.exists());

        reset_global_state(&state);
    }

    #[test]
    fn cleanup_removed_files_removes_output_and_diagnostics() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let state = SiteIndex::new();
        reset_global_state(&state);

        let source = crate::utils::path::normalize_path(&config.build.content.join("removed.md"));
        let url = UrlPath::from_page("/removed/");
        let output_file = output_file_for(&config, &url);
        fs::create_dir_all(output_file.parent().unwrap()).unwrap();
        fs::write(&output_file, "stale output").unwrap();
        state.with_pages(|pages| {
            PageState::new(pages).record_links(&url, vec![UrlPath::from_page("/target/")]);
        });

        let rel = source
            .strip_prefix(config.get_root())
            .unwrap_or(&source)
            .display()
            .to_string();
        let mut diagnostics = PersistedDiagnostics::new();
        diagnostics.push_error(PersistedError::new(&rel, url.to_string(), "old error"));
        diagnostics.set_warnings(&rel, vec!["old warning".to_string()]);

        let removed = vec![RemovedFile {
            source_path: source,
            url_path: url.clone(),
        }];
        cleanup_removed_files(&removed, &config, &state, &mut diagnostics);

        assert!(!output_file.exists());
        assert!(state.with_pages(|pages| PageState::new(pages).links_to(&url).is_empty()));
        assert_eq!(diagnostics.error_count(), 0);
        assert_eq!(diagnostics.warning_count(), 0);

        reset_global_state(&state);
    }

    #[test]
    fn startup_batch_permalink_change_cleans_old_output() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let state = SiteIndex::new();
        reset_global_state(&state);

        let source = config.build.content.join("post.md");
        write_markdown_with_permalink(&source, "Permalink Changed", "/new-url/");
        let source = crate::utils::path::normalize_path(&source);

        let old_url = UrlPath::from_page("/legacy/");
        let new_url = UrlPath::from_page("/new-url/");

        let old_output = output_file_for(&config, &old_url);
        fs::create_dir_all(old_output.parent().unwrap()).unwrap();
        fs::write(&old_output, "stale old output").unwrap();
        state.with_pages(|pages| {
            PageState::new(pages).record_links(&old_url, vec![UrlPath::from_page("/target/")]);
        });

        let mut cached_urls = FxHashMap::default();
        cached_urls.insert(source.clone(), old_url.clone());
        let mut diagnostics = PersistedDiagnostics::new();
        let host = typst_host(&config);
        let ready = ready();

        let stats = compile_startup_batch(
            std::slice::from_ref(&source),
            &cached_urls,
            &config,
            &host,
            &state,
            &mut diagnostics,
            &ready,
        );

        let new_output = output_file_for(&config, &new_url);
        assert_eq!(stats.success, 1);
        assert_eq!(stats.failed, 0);
        assert!(
            new_output.exists(),
            "new permalink output should be written"
        );
        assert!(
            !old_output.exists(),
            "old permalink output should be removed"
        );
        assert!(state.with_pages(|pages| PageState::new(pages).links_to(&old_url).is_empty()));

        reset_global_state(&state);
    }
}
