//! Site validation command.

mod report;
mod scan;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;
use rayon::prelude::*;

use super::common::collect_content_files;
use crate::address::SiteIndex;
use crate::compiler::page::CompiledPage;
use crate::compiler::page::typst::{MAX_METADATA_SCAN_ITERATIONS, scan_single_with_current};
use crate::config::SiteConfig;
use crate::core::{ContentKind, LinkKind, LinkOrigin, ResolveContext, ResolveResult};
use crate::log;
use crate::package::build_visible_inputs;
use crate::page::{HashStabilityTracker, PageKind, PageMeta, StabilityDecision, StoredPageMap};
use crate::utils::path::route::{strip_path_prefix, strip_path_prefix_in_text};
use crate::utils::{plural_count, plural_s};

use report::ValidationReport;
use scan::scan_markdown;

/// Result type for address space building: (pages, typst_links, compile_errors)
type AddressSpaceResult = (
    Vec<CompiledPage>,
    HashMap<PathBuf, Vec<scan::ScannedLink>>,
    Vec<(String, String)>,
);

/// Result type for batch Typst scanning: (metas, links, errors)
type BatchScanResult = (
    Vec<Option<serde_json::Value>>,
    Vec<Vec<scan::ScannedLink>>,
    Vec<(String, String)>,
);

/// Parsed scan result with PageMeta instead of raw JSON
type ParsedScanResult = (
    Vec<Option<PageMeta>>,
    Vec<Vec<scan::ScannedLink>>,
    Vec<(String, String)>,
);

/// Validate site links and assets
pub fn validate_site(config: &SiteConfig) -> Result<()> {
    let state = SiteIndex::new();

    // Register VFS with nested asset mappings (no font warmup needed)
    let nested_mappings =
        crate::compiler::page::typst::build_nested_mappings(&config.build.assets.nested);
    crate::compiler::page::typst::init_vfs(config.get_root().to_path_buf(), nested_mappings);

    let args = get_validate_args();
    let files = collect_content_files(&args.paths, &config.build.content)?;

    if files.is_empty() {
        log!("validate"; "no content files found");
        return Ok(());
    }

    let file_count = files.len();
    let validate_config = &config.validate;

    // Check if any validation is enabled
    let check_pages = validate_config.pages.enable;
    let check_assets = validate_config.assets.enable;

    if !check_pages && !check_assets {
        log!("validate"; "no checks enabled");
        return Ok(());
    }

    log!("validate"; "validating {}", plural_count(file_count, "file"));

    // Setup paths
    let root = crate::utils::path::normalize_path(config.get_root());

    // Unified report
    let report = Arc::new(RwLock::new(ValidationReport::default()));

    // Build AddressSpace for validation (unified scan: metadata + links + errors)
    let (all_pages, typst_links) = if check_pages || check_assets {
        let (pages, links, compile_errors) = build_address_space(config, &state)?;

        // Add compile errors to report as asset errors
        // Extract path from "file not found (searched at /abs/path)" -> "/relative/path"
        for (source, error) in compile_errors {
            let path = extract_asset_path(&error, &root);
            report
                .write()
                .add_asset(source, format!("`{}`", path), "not found".to_string());
        }

        (pages, links)
    } else {
        (Vec::new(), HashMap::new())
    };

    // Check for permalink conflicts
    let url_sources = crate::address::conflict::collect_url_sources(&all_pages, config);
    let conflicts = crate::address::conflict::detect_conflicts(&url_sources, config.get_root());
    if !conflicts.is_empty() {
        let prefix = config.paths().prefix().to_string_lossy().into_owned();
        crate::address::conflict::print_conflicts_with_prefix(&conflicts, &prefix);
        let total_sources: usize = conflicts.iter().map(|c| c.sources.len()).sum();
        anyhow::bail!(
            "validation failed: {} conflicting url{}, {} source{}",
            conflicts.len(),
            plural_s(conflicts.len()),
            total_sources,
            plural_s(total_sources)
        );
    }

    // Validate links (Typst links from unified scan, Markdown scanned separately)
    validate_all_links(
        &files,
        &root,
        config,
        &state,
        &all_pages,
        &typst_links,
        &report,
    );

    // Log page link results
    if check_pages {
        let count = report.read().page_error_count();
        if count > 0 {
            log!("validate"; "found {} broken page link{}", count, plural_s(count));
        } else {
            log!("validate"; "all page links valid");
        }
    }

    // Log asset results
    if check_assets {
        let count = report.read().asset_error_count();
        if count > 0 {
            log!("validate"; "found {} broken asset link{}", count, plural_s(count));
        } else {
            log!("validate"; "all asset links valid");
        }
    }

    // Get final report
    let report = Arc::try_unwrap(report).unwrap().into_inner();

    // Print detailed report (pages -> assets)
    report.print();

    // Final summary (pages -> assets)
    print_summary(report.page_file_count(), report.asset_file_count())
}

/// Validate all links using pre-scanned Typst links and scanning Markdown files
fn validate_all_links(
    files: &[PathBuf],
    root: &std::path::Path,
    config: &SiteConfig,
    state: &SiteIndex,
    all_pages: &[CompiledPage],
    typst_links: &HashMap<PathBuf, Vec<scan::ScannedLink>>,
    report: &Arc<RwLock<ValidationReport>>,
) {
    // Collect all nested asset source directories with their output prefixes
    let nested_assets: Vec<_> = config
        .build
        .assets
        .nested
        .iter()
        .map(|e| (e.output_name().to_string(), root.join(e.source())))
        .collect();

    // Collect all flatten files (for asset link validation)
    let flatten_outputs: Vec<_> = config
        .build
        .assets
        .flatten
        .iter()
        .filter(|e| e.source().exists())
        .map(|e| e.output_name().to_string())
        .collect();

    // Process Typst links (already scanned in build_address_space)
    for (file, links) in typst_links {
        let source = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .to_string();

        validate_links(
            &source,
            file,
            links,
            config,
            all_pages,
            report,
            root,
            &nested_assets,
            &flatten_outputs,
            state,
        );
    }

    // Separate Markdown files and scan them
    let (_, markdown_files) = ContentKind::partition_by_kind(files);

    // Process Markdown files in parallel.
    let markdown_results: Vec<_> = state.with_pages(|store| {
        markdown_files
            .par_iter()
            .filter_map(|file| {
                scan_markdown(file, root, config, store)
                    .ok()
                    .map(|result| ((*file).clone(), result))
            })
            .collect()
    });

    for (file, result) in markdown_results {
        validate_links(
            &result.source,
            &file,
            &result.links,
            config,
            all_pages,
            report,
            root,
            &nested_assets,
            &flatten_outputs,
            state,
        );
    }
}

/// Validate links from a single file
#[allow(clippy::too_many_arguments)]
fn validate_links(
    source: &str,
    file: &std::path::Path,
    links: &[scan::ScannedLink],
    config: &SiteConfig,
    all_pages: &[CompiledPage],
    report: &Arc<RwLock<ValidationReport>>,
    root: &std::path::Path,
    nested_assets: &[(String, PathBuf)],
    flatten_outputs: &[String],
    state: &SiteIndex,
) {
    let prefix = config.paths().prefix().to_string_lossy().into_owned();
    let validate_config = &config.validate;

    // Find the page for this file (needed for ResolveContext)
    let page = all_pages.iter().find(|p| p.route.source == *file);

    for link in links {
        // Determine if this is an asset attribute (src, poster, data, Image)
        let is_asset_attr = link.origin.is_asset_attr();

        match link.kind() {
            // External links: skip (no HTTP validation)
            LinkKind::External(_) => {}

            // Site-root links: could be page OR static asset
            LinkKind::SiteRoot(path) => {
                // For asset attributes, check all static assets directories
                if is_asset_attr {
                    let trimmed = path.trim_start_matches('/');

                    // Check nested assets: /images/xxx -> find entry with output_name "images"
                    let in_nested = nested_assets.iter().any(|(output_name, abs_source)| {
                        // Exact match: /images -> output_name "images"
                        if trimmed == output_name {
                            return abs_source.exists();
                        }
                        // Prefix with slash: /images/xxx -> output_name "images", rest "xxx"
                        if let Some(rest) = trimmed.strip_prefix(output_name)
                            && let Some(rest) = rest.strip_prefix('/')
                        {
                            return abs_source.join(rest).exists();
                        }
                        false
                    });

                    // Check flatten outputs (e.g., /favicon.ico -> "favicon.ico")
                    let in_flatten = flatten_outputs.iter().any(|name| trimmed == name);

                    if in_nested || in_flatten {
                        continue;
                    }

                    // Asset not found via nested/flatten - check if it exists in source
                    // and suggest the correct path
                    if validate_config.assets.enable {
                        // Check if path matches a nested source directory
                        // e.g., /assets/images/photo.webp -> should be /images/photo.webp
                        let suggestion =
                            nested_assets.iter().find_map(|(output_name, abs_source)| {
                                // Get relative source path by stripping root
                                let rel_source = abs_source.strip_prefix(root).ok()?;
                                let source_str = rel_source.to_string_lossy();
                                // trimmed: "assets/images/photo.webp", source_str: "assets/images"
                                let rest = trimmed.strip_prefix(source_str.as_ref())?;
                                let rest = rest.trim_start_matches('/');
                                let file_path = abs_source.join(rest);
                                if file_path.exists() {
                                    let correct = if rest.is_empty() {
                                        format!("/{}", output_name)
                                    } else {
                                        format!("/{}/{}", output_name, rest)
                                    };
                                    return Some(correct);
                                }
                                None
                            });

                        let reason = if let Some(correct_path) = suggestion {
                            format!("maybe should be `{}`", correct_path)
                        } else {
                            "not found".to_string()
                        };

                        report.write().add_asset(
                            source.to_string(),
                            format!("`{}`", link.dest),
                            reason,
                        );
                    }
                    continue;
                }

                // Try AddressSpace for non-asset links
                if !validate_config.pages.enable {
                    continue;
                }

                let Some(page) = page else { continue };

                let ctx = ResolveContext {
                    current_permalink: &page.route.permalink,
                    source_path: &page.route.source,
                    origin: link.origin,
                };

                // Mirror compile-time link normalization (prefix + slug) so
                // validation targets match final emitted URLs.
                let resolved_link =
                    crate::pipeline::transform::resolve_link(&link.dest, config, &page.route)
                        .unwrap_or_else(|_| link.dest.clone());

                let result = state.read(|_, space| {
                    let mut result = space.resolve(&resolved_link, &ctx);
                    // Fallback for mixed prefixed/unprefixed permalink states.
                    if matches!(result, ResolveResult::NotFound { .. })
                        && resolved_link != link.dest
                    {
                        result = space.resolve(&link.dest, &ctx);
                    }
                    result
                });
                handle_resolve_result(
                    result,
                    source,
                    &link.dest,
                    is_asset_attr,
                    validate_config,
                    report,
                    &prefix,
                );
            }

            // File-relative and fragment links: validate via AddressSpace
            LinkKind::FileRelative(_) | LinkKind::Fragment(_) => {
                if !validate_config.pages.enable && !validate_config.assets.enable {
                    continue;
                }

                let Some(page) = page else { continue };

                let ctx = ResolveContext {
                    current_permalink: &page.route.permalink,
                    source_path: &page.route.source,
                    origin: link.origin,
                };

                let result = state.read(|_, space| space.resolve(&link.dest, &ctx));
                handle_resolve_result(
                    result,
                    source,
                    &link.dest,
                    is_asset_attr,
                    validate_config,
                    report,
                    &prefix,
                );
            }
        }
    }
}

/// Handle AddressSpace resolve result
fn handle_resolve_result(
    result: ResolveResult,
    source: &str,
    link: &str,
    is_asset_attr: bool,
    validate_config: &crate::config::ValidateConfig,
    report: &Arc<RwLock<ValidationReport>>,
    prefix: &str,
) {
    let display_link = if link.starts_with('/') {
        strip_path_prefix(link, prefix)
    } else {
        link.to_string()
    };

    match result {
        ResolveResult::Found(_) | ResolveResult::External(_) => {}

        ResolveResult::NotFound { .. } => {
            if is_asset_attr {
                if validate_config.assets.enable {
                    report.write().add_asset(
                        source.to_string(),
                        display_link.clone(),
                        "not found".to_string(),
                    );
                }
            } else if validate_config.pages.enable {
                report.write().add_page(
                    source.to_string(),
                    display_link.clone(),
                    "not found".to_string(),
                );
            }
        }

        ResolveResult::FragmentNotFound {
            fragment,
            available,
            ..
        } => {
            if validate_config.pages.enable {
                let msg = if available.is_empty() {
                    format!("fragment '{}' not found", fragment)
                } else {
                    format!(
                        "fragment '{}' not found (available: {})",
                        fragment,
                        available.join(", ")
                    )
                };
                report.write().add_page(
                    source.to_string(),
                    display_link.clone(),
                    strip_path_prefix_in_text(&msg, prefix),
                );
            }
        }

        ResolveResult::Warning { message, .. } => {
            if validate_config.pages.enable {
                report.write().add_page(
                    source.to_string(),
                    display_link.clone(),
                    strip_path_prefix_in_text(&message, prefix),
                );
            }
        }

        ResolveResult::Error { message } => {
            if validate_config.pages.enable {
                report.write().add_page(
                    source.to_string(),
                    display_link,
                    strip_path_prefix_in_text(&message, prefix),
                );
            }
        }
    }
}

/// Build AddressSpace using fast batch scanning (no full compilation)
/// Returns (pages, typst_links, compile_errors)
/// - pages: All compiled pages
/// - typst_links: HashMap<PathBuf, Vec<ScannedLink>> for Typst files
/// - compile_errors: Vec<(source_path, error_message)> for compile failures
fn build_address_space(config: &SiteConfig, state: &SiteIndex) -> Result<AddressSpaceResult> {
    use rayon::prelude::*;

    use crate::cli::common::scan_markdown_file;

    let content_files = crate::compiler::collect_all_files(&config.build.content);
    let label = &config.build.meta.label;

    // Separate Typst and Markdown files
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    state.clear();
    let (pages, typst_links, compile_errors) =
        state.with_pages(|store| -> Result<AddressSpaceResult> {
            // Preload markdown metadata into the page store so @tola/pages has complete
            // cross-format context during Typst validation scans.
            for file in &markdown_files {
                if let Ok(result) = scan_markdown_file(file, config, store)
                    && let Some(meta_json) = result.raw_meta
                {
                    update_stored_page_from_meta(file, &meta_json, config, store);
                }
            }

            // Batch scan Typst files for metadata AND links (unified scan)
            let (typst_metas, typst_links_vec, compile_errors): ParsedScanResult =
                if typst_files.is_empty() {
                    (vec![], vec![], vec![])
                } else {
                    let (metas, links, errors) =
                        batch_scan_typst_unified(&typst_files, label, config, store)?;
                    let parsed_metas = metas
                        .into_iter()
                        .map(|json| json.and_then(|j| serde_json::from_value(j).ok()))
                        .collect();
                    (parsed_metas, links, errors)
                };

            // Build CompiledPage for Typst files and collect links
            let mut pages: Vec<CompiledPage> =
                Vec::with_capacity(typst_files.len() + markdown_files.len());
            let mut typst_links: HashMap<PathBuf, Vec<scan::ScannedLink>> =
                HashMap::with_capacity(typst_files.len());

            for ((file, meta), links) in typst_files.iter().zip(typst_metas).zip(typst_links_vec) {
                if let Ok(page) = CompiledPage::from_paths_with_meta(file, config, meta) {
                    typst_links.insert(page.route.source.clone(), links);
                    pages.push(page);
                }
            }

            // Process Markdown files in parallel
            let markdown_pages: Vec<CompiledPage> = markdown_files
                .par_iter()
                .filter_map(|file| {
                    let meta = scan_markdown_file(file, config, store)
                        .ok()
                        .and_then(|result| result.raw_meta)
                        .and_then(|json| serde_json::from_value(json).ok());

                    CompiledPage::from_paths_with_meta(*file, config, meta).ok()
                })
                .collect();

            pages.extend(markdown_pages);
            Ok((pages, typst_links, compile_errors))
        })?;

    // Build the global address space
    crate::compiler::page::build_address_space(&pages, config, state);

    Ok((pages, typst_links, compile_errors))
}

/// Batch scan Typst files for metadata AND links (unified scan)
/// Returns (metas, links, errors) where:
/// - metas: Vec<Option<JsonValue>> for each file
/// - links: Vec<Vec<ScannedLink>> for each file
/// - errors: Vec<(source_path, error_message)> for compile failures
fn batch_scan_typst_unified(
    files: &[&PathBuf],
    label: &str,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<BatchScanResult> {
    use typst_batch::prelude::*;

    if files.is_empty() {
        return Ok((vec![], vec![], vec![]));
    }

    let root = crate::utils::path::normalize_path(config.get_root());

    // Validate scan follows visible-phase contract for virtual package injection.
    // It shares base inputs and adds per-file @tola/current only for iterative pages.
    let base_inputs = build_visible_inputs(config, store)?;
    let scanner = Batcher::for_scan(&root)
        .with_inputs_obj(base_inputs)
        .with_snapshot_from(files)?;

    match scanner.batch_scan(files) {
        Ok(results) => {
            let mut metas = Vec::with_capacity(results.len());
            let mut all_links = Vec::with_capacity(results.len());
            let mut errors = Vec::new();
            let mut iterative_indices = Vec::new();

            for (index, (result, file)) in results.into_iter().zip(files).enumerate() {
                let rel_path = file
                    .strip_prefix(&root)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string();

                match result {
                    Ok(scan) => {
                        let meta = scan.metadata(label);
                        if let Some(ref meta_json) = meta {
                            update_stored_page_from_meta(file, meta_json, config, store);
                        }
                        if PageKind::from_packages(scan.accessed_packages()).is_iterative() {
                            iterative_indices.push(index);
                        }
                        metas.push(meta);
                        // Extract links and convert to ScannedLink.
                        let links: Vec<scan::ScannedLink> = scan
                            .links()
                            .into_iter()
                            .map(|l| scan::ScannedLink {
                                dest: l.dest,
                                origin: LinkOrigin::from(l.source),
                            })
                            .collect();
                        all_links.push(links);
                    }
                    Err(e) => {
                        // Retry with per-file @tola/current context so pages
                        // using current.permalink/path in body can scan.
                        match scan_single_with_current(&root, file, config, store) {
                            Ok(scan) => {
                                let meta = scan.metadata(label);
                                if let Some(ref meta_json) = meta {
                                    update_stored_page_from_meta(file, meta_json, config, store);
                                }
                                if PageKind::from_packages(scan.accessed_packages()).is_iterative()
                                {
                                    iterative_indices.push(index);
                                }
                                metas.push(meta);
                                let links: Vec<scan::ScannedLink> = scan
                                    .links()
                                    .into_iter()
                                    .map(|l| scan::ScannedLink {
                                        dest: l.dest,
                                        origin: LinkOrigin::from(l.source),
                                    })
                                    .collect();
                                all_links.push(links);
                            }
                            Err(_) => {
                                // Keep original compile error for diagnostics.
                                errors.push((rel_path, e.to_string()));
                                metas.push(None);
                                all_links.push(vec![]);
                            }
                        }
                    }
                }
            }

            if iterative_indices.is_empty() {
                return Ok((metas, all_links, errors));
            }

            let mut stability =
                HashStabilityTracker::with_oscillation_detection(store.pages_hash());
            for iteration in 0..MAX_METADATA_SCAN_ITERATIONS {
                for &idx in &iterative_indices {
                    let file = files[idx];
                    let rel_path = file
                        .strip_prefix(&root)
                        .unwrap_or(file)
                        .to_string_lossy()
                        .to_string();
                    let scan = match scan_single_with_current(&root, file, config, store) {
                        Ok(scan) => scan,
                        Err(e) => {
                            errors.push((rel_path, e.to_string()));
                            continue;
                        }
                    };
                    let meta = scan.metadata(label);
                    if let Some(ref meta_json) = meta {
                        update_stored_page_from_meta(file, meta_json, config, store);
                    }
                    metas[idx] = meta;
                    all_links[idx] = scan
                        .links()
                        .into_iter()
                        .map(|l| scan::ScannedLink {
                            dest: l.dest,
                            origin: LinkOrigin::from(l.source),
                        })
                        .collect();
                }

                match stability.decide(store.pages_hash(), iteration, MAX_METADATA_SCAN_ITERATIONS)
                {
                    StabilityDecision::Converged => {
                        crate::debug!(
                            "validate";
                            "metadata converged after {} iteration(s)",
                            iteration + 1
                        );
                        break;
                    }
                    StabilityDecision::Oscillating => {
                        crate::log!(
                            "warning";
                            "validate metadata oscillating (cycle detected), stopping after {} iterations",
                            iteration + 1
                        );
                        break;
                    }
                    StabilityDecision::MaxIterationsReached => {
                        crate::log!(
                            "warning";
                            "validate metadata did not converge after {} iterations",
                            MAX_METADATA_SCAN_ITERATIONS
                        );
                    }
                    StabilityDecision::Continue => {}
                }
            }

            Ok((metas, all_links, errors))
        }
        Err(e) => {
            anyhow::bail!("Batch scan failed: {}", e);
        }
    }
}

fn update_stored_page_from_meta(
    file: &std::path::Path,
    meta_json: &serde_json::Value,
    config: &SiteConfig,
    store: &StoredPageMap,
) {
    let Ok(page_meta) = serde_json::from_value::<PageMeta>(meta_json.clone()) else {
        return;
    };
    let _ = store.apply_meta_for_source(file, page_meta, config);
}

/// Extract asset path from compile error.
/// "file not found (searched at /abs/root/images/photo.webp)" -> "/images/photo.webp"
fn extract_asset_path(error: &str, root: &std::path::Path) -> String {
    // Find "searched at " and extract the path
    if let Some(start) = error.find("searched at ") {
        let path_start = start + "searched at ".len();
        let path_end = error[path_start..]
            .find(')')
            .map(|e| path_start + e)
            .unwrap_or(error.len());
        let abs_path = &error[path_start..path_end];

        // Convert absolute path to site-relative path
        let root_str = root.to_string_lossy();
        if let Some(rel) = abs_path.strip_prefix(root_str.as_ref()) {
            return format!("/{}", rel.trim_start_matches('/'));
        }
        return abs_path.to_string();
    }
    // Fallback: return error as-is
    error.to_string()
}

/// Print final summary and return error if validation failed
fn print_summary(page_errors: usize, asset_errors: usize) -> Result<()> {
    if page_errors > 0 || asset_errors > 0 {
        let mut parts = Vec::new();
        if page_errors > 0 {
            parts.push(format!(
                "{} with page link errors",
                plural_count(page_errors, "file")
            ));
        }
        if asset_errors > 0 {
            parts.push(format!(
                "{} with asset link errors",
                plural_count(asset_errors, "file")
            ));
        }
        anyhow::bail!("found {}", parts.join(", "));
    }

    Ok(())
}

fn get_validate_args() -> crate::cli::ValidateArgs {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    let cli = Cli::parse();
    match cli.command {
        Commands::Validate { args } => args,
        _ => crate::cli::ValidateArgs {
            paths: vec![],
            warn_only: false,
            pages: None,
            assets: None,
        },
    }
}
