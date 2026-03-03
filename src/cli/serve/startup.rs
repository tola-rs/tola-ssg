use std::path::{Path, PathBuf};

use anyhow::Result;
use rustc_hash::{FxHashMap, FxHashSet};
use tola_vdom::CacheKey;

use super::{bind_server, init_serve_build, scan_pages, serve_build, set_scan_ready};
use crate::address::GLOBAL_ADDRESS_SPACE;
use crate::cache::{self, PersistedDiagnostics, PersistedError, RemovedFile};
use crate::compiler::dependency::{self, collect_virtual_dependents};
use crate::compiler::page::{BUILD_CACHE, cache_vdom};
use crate::compiler::scheduler::SCHEDULER;
use crate::config::{self, SiteConfig, clear_clean_flag};
use crate::core::UrlPath;
use crate::page::{PAGE_LINKS, STORED_PAGES};
use crate::reload::compile::{self, CompileOutcome};
use crate::{debug, log, logger};

/// Start serve with cached build support
pub fn serve_with_cache(config: &SiteConfig) -> Result<()> {
    use crate::core::{set_healthy, set_serving};

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

    let bound_server = bind_server()?;

    SCHEDULER.start_workers();
    set_scan_ready(false);

    let config_arc = config::cfg();
    let needs_full_build = !has_cache;
    std::thread::spawn(move || {
        let scan_success = !needs_full_build || progressive_scan(&config_arc);

        if !scan_success {
            set_scan_ready(false);
            if needs_full_build {
                set_serving();
            }
            set_healthy(false);
            return;
        }

        // Full-build path uses progressive_scan() before serve_build(),
        // so URL->source mapping is now available for on-demand requests.
        if needs_full_build {
            set_scan_ready(true);
        }

        let build_success = if needs_full_build {
            match serve_build(&config_arc) {
                Ok(_) => true,
                Err(e) => {
                    log!("build"; "initial build failed: {}", e);
                    false
                }
            }
        } else {
            startup_with_cache(&config_arc)
        };

        set_healthy(build_success);

        if build_success {
            clear_clean_flag();
        }

        if has_cache || needs_full_build {
            set_serving();
        }
    });

    bound_server.run()
}

fn progressive_scan(config: &SiteConfig) -> bool {
    use crate::core::is_shutdown;

    if let Err(e) = init_serve_build(config) {
        debug!("init"; "failed: {}", e);
        return false;
    }

    if is_shutdown() {
        return false;
    }

    if let Err(e) = scan_pages(config) {
        debug!("scan"; "failed: {}", e);
        return false;
    }

    if is_shutdown() {
        return false;
    }

    true
}

fn startup_with_cache(config: &SiteConfig) -> bool {
    if let Err(e) = init_serve_build(config) {
        log!("build"; "cache startup init failed: {}", e);
        set_scan_ready(false);
        return false;
    }

    if let Err(e) = scan_pages(config) {
        log!("scan"; "cache startup scan failed: {}", e);
        set_scan_ready(false);
        return false;
    }
    set_scan_ready(true);

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

    cleanup_removed_files(&modified.removed, config, &mut diagnostics);

    for path in modified.created {
        files_to_compile.insert(path);
    }
    for path in modified.modified {
        files_to_compile.insert(path);
    }

    let mut compile_targets: Vec<_> = files_to_compile.into_iter().collect();
    compile_targets.sort();

    let pages_hash = STORED_PAGES.pages_hash();
    let mut stats = StartupCompileStats::default();
    if !compile_targets.is_empty() {
        stats = compile_startup_batch(
            &compile_targets,
            &modified.cached_urls_by_source,
            config,
            &mut diagnostics,
        );
    }

    if STORED_PAGES.pages_hash() != pages_hash {
        let dependents = collect_virtual_dependents();
        if !dependents.is_empty() {
            let virtual_stats = compile_startup_batch(
                &dependents.into_iter().collect::<Vec<_>>(),
                &FxHashMap::default(),
                config,
                &mut diagnostics,
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
        logger::WatchStatus::new().error(&first_error.path, &first_error.error);
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

    true
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
    diagnostics: &mut PersistedDiagnostics,
) {
    if removed.is_empty() {
        return;
    }

    for item in removed {
        SCHEDULER.invalidate(&item.source_path);
        GLOBAL_ADDRESS_SPACE
            .write()
            .remove_by_source(&item.source_path);
        STORED_PAGES.remove_by_source(&item.source_path);
        dependency::remove_content(&item.source_path);
        cleanup_url_artifacts(config, &item.url_path);

        let rel = relative_source_path(config, &item.source_path);
        diagnostics.clear_for(&rel);
    }
}

fn cleanup_url_artifacts(config: &SiteConfig, url: &UrlPath) {
    BUILD_CACHE.remove(&CacheKey::new(url.as_str()));
    PAGE_LINKS.record(url, vec![]);
    compile::cleanup_output_for_url(config, url);
}

fn cleanup_cached_url(
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    source_path: &Path,
    config: &SiteConfig,
) {
    if let Some(old_url) = cached_urls.get(source_path) {
        cleanup_url_artifacts(config, old_url);
    }
}

fn cleanup_cached_url_if_changed(
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    source_path: &Path,
    new_url: &UrlPath,
    config: &SiteConfig,
) {
    if let Some(old_url) = cached_urls.get(source_path)
        && old_url != new_url
    {
        cleanup_url_artifacts(config, old_url);
    }
}

fn relative_source_path(config: &SiteConfig, path: &Path) -> String {
    path.strip_prefix(config.get_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn handle_startup_vdom_outcome(
    path: PathBuf,
    url_path: UrlPath,
    vdom: Box<tola_vdom::Document<crate::compiler::family::Indexed>>,
    warnings: Vec<String>,
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    config: &SiteConfig,
    diagnostics: &mut PersistedDiagnostics,
) {
    cleanup_cached_url_if_changed(cached_urls, &path, &url_path, config);
    cache_vdom(&url_path, *vdom);

    let rel = relative_source_path(config, &path);
    diagnostics.clear_errors_for(&rel);
    diagnostics.set_warnings(&rel, warnings);
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
    diagnostics: &mut PersistedDiagnostics,
) {
    cleanup_cached_url(cached_urls, input_path, config);
    diagnostics.clear_for(rel_input);
}

fn compile_startup_batch(
    paths: &[PathBuf],
    cached_urls: &FxHashMap<PathBuf, UrlPath>,
    config: &SiteConfig,
    diagnostics: &mut PersistedDiagnostics,
) -> StartupCompileStats {
    let mut stats = StartupCompileStats::default();
    let outcomes = compile::compile_startup_batch(paths, config);

    for (input_path, outcome) in paths.iter().zip(outcomes.into_iter()) {
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
            } => {
                handle_startup_vdom_outcome(
                    path,
                    url_path,
                    vdom,
                    warnings,
                    cached_urls,
                    config,
                    diagnostics,
                );
                stats.success += 1;
            }
            CompileOutcome::Error {
                path,
                url_path,
                error,
            } => {
                handle_startup_error_outcome(path, url_path, error, config, diagnostics);
                stats.failed += 1;
            }
            CompileOutcome::Skipped => {
                handle_startup_skipped_outcome(
                    input_path,
                    &rel_input,
                    cached_urls,
                    config,
                    diagnostics,
                );
                stats.skipped += 1;
            }
            CompileOutcome::Reload { reason } => {
                debug!("startup"; "startup compile requested reload: {}", reason);
                diagnostics.clear_for(&rel_input);
                stats.skipped += 1;
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

    fn reset_global_state() {
        BUILD_CACHE.clear();
        PAGE_LINKS.clear();
        STORED_PAGES.clear();
        GLOBAL_ADDRESS_SPACE.write().clear();
        dependency::clear_graph();
        freshness::clear_cache();
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

    #[test]
    fn startup_batch_skipped_draft_cleans_cached_output() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        reset_global_state();

        let source = config.build.content.join("post.md");
        write_markdown(&source, "Draft Post", true);
        let source = crate::utils::path::normalize_path(&source);

        let old_url = UrlPath::from_page("/legacy/");
        let output_file = output_file_for(&config, &old_url);
        fs::create_dir_all(output_file.parent().unwrap()).unwrap();
        fs::write(&output_file, "stale output").unwrap();
        PAGE_LINKS.record(&old_url, vec![UrlPath::from_page("/target/")]);

        let rel = config.root_relative(&source).display().to_string();
        let mut diagnostics = PersistedDiagnostics::new();
        diagnostics.push_error(PersistedError::new(&rel, old_url.to_string(), "old error"));
        diagnostics.set_warnings(&rel, vec!["old warning".to_string()]);

        let mut cached_urls = FxHashMap::default();
        cached_urls.insert(source.clone(), old_url.clone());

        let stats = compile_startup_batch(
            std::slice::from_ref(&source),
            &cached_urls,
            &config,
            &mut diagnostics,
        );

        assert_eq!(stats.success, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.skipped, 1);
        assert!(!output_file.exists(), "stale output should be removed");
        assert!(PAGE_LINKS.links_to(&old_url).is_empty());
        assert_eq!(diagnostics.error_count(), 0);
        assert_eq!(diagnostics.warning_count(), 0);

        reset_global_state();
    }

    #[test]
    fn startup_batch_success_writes_updated_html() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        reset_global_state();

        let source = config.build.content.join("post.md");
        write_markdown(&source, "Startup Modified", false);
        let source = crate::utils::path::normalize_path(&source);

        let rel = config.root_relative(&source).display().to_string();
        let mut diagnostics = PersistedDiagnostics::new();
        diagnostics.push_error(PersistedError::new(&rel, "/post/", "old compile error"));

        let stats = compile_startup_batch(
            std::slice::from_ref(&source),
            &FxHashMap::default(),
            &config,
            &mut diagnostics,
        );

        let output_file = output_file_for(&config, &UrlPath::from_page("/post/"));
        let html = fs::read_to_string(&output_file).unwrap_or_default();

        assert_eq!(stats.success, 1);
        assert_eq!(stats.failed, 0);
        assert!(output_file.exists());
        assert!(html.contains("Startup Modified"));
        assert_eq!(diagnostics.error_count(), 0);

        reset_global_state();
    }

    #[test]
    fn cleanup_removed_files_removes_output_and_diagnostics() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        reset_global_state();

        let source = crate::utils::path::normalize_path(&config.build.content.join("removed.md"));
        let url = UrlPath::from_page("/removed/");
        let output_file = output_file_for(&config, &url);
        fs::create_dir_all(output_file.parent().unwrap()).unwrap();
        fs::write(&output_file, "stale output").unwrap();
        PAGE_LINKS.record(&url, vec![UrlPath::from_page("/target/")]);

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
        cleanup_removed_files(&removed, &config, &mut diagnostics);

        assert!(!output_file.exists());
        assert!(PAGE_LINKS.links_to(&url).is_empty());
        assert_eq!(diagnostics.error_count(), 0);
        assert_eq!(diagnostics.warning_count(), 0);

        reset_global_state();
    }

    #[test]
    fn startup_batch_permalink_change_cleans_old_output() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        reset_global_state();

        let source = config.build.content.join("post.md");
        write_markdown_with_permalink(&source, "Permalink Changed", "/new-url/");
        let source = crate::utils::path::normalize_path(&source);

        let old_url = UrlPath::from_page("/legacy/");
        let new_url = UrlPath::from_page("/new-url/");

        let old_output = output_file_for(&config, &old_url);
        fs::create_dir_all(old_output.parent().unwrap()).unwrap();
        fs::write(&old_output, "stale old output").unwrap();
        PAGE_LINKS.record(&old_url, vec![UrlPath::from_page("/target/")]);

        let mut cached_urls = FxHashMap::default();
        cached_urls.insert(source.clone(), old_url.clone());
        let mut diagnostics = PersistedDiagnostics::new();

        let stats = compile_startup_batch(
            std::slice::from_ref(&source),
            &cached_urls,
            &config,
            &mut diagnostics,
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
        assert!(PAGE_LINKS.links_to(&old_url).is_empty());

        reset_global_state();
    }
}
