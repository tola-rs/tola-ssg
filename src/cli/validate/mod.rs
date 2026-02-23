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
use crate::compiler::page::CompiledPage;
use crate::config::SiteConfig;
use crate::core::{ContentKind, GLOBAL_ADDRESS_SPACE, LinkKind, ResolveContext, ResolveResult};
use crate::log;
use crate::utils::{plural_count, plural_s};

use report::ValidationReport;
use scan::scan_markdown;

/// Validate site links and assets
pub fn validate_site(config: &SiteConfig) -> Result<()> {
    // Register VFS with nested asset mappings (no font warmup needed)
    let nested_mappings =
        crate::compiler::page::typst::init::build_nested_mappings(&config.build.assets.nested);
    crate::compiler::page::typst::init::init_vfs_with_mappings(
        config.get_root().to_path_buf(),
        nested_mappings,
    );

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
        let (pages, links, compile_errors) = build_address_space(config)?;

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
        crate::address::conflict::print_conflicts(&conflicts);
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
    validate_all_links(&files, &root, config, &all_pages, &typst_links, &report);

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
    all_pages: &[CompiledPage],
    typst_links: &HashMap<PathBuf, Vec<scan::ScannedLink>>,
    report: &Arc<RwLock<ValidationReport>>,
) {
    let validate_config = &config.validate;

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
            validate_config,
            all_pages,
            report,
            root,
            &nested_assets,
            &flatten_outputs,
        );
    }

    // Separate Markdown files and scan them
    let (_, markdown_files) = ContentKind::partition_by_kind(files);

    // Process Markdown files in parallel
    markdown_files.par_iter().for_each(|file| {
        if let Ok(result) = scan_markdown(file, root, config) {
            validate_links(
                &result.source,
                file,
                &result.links,
                validate_config,
                all_pages,
                report,
                root,
                &nested_assets,
                &flatten_outputs,
            );
        }
    });
}

/// Validate links from a single file
#[allow(clippy::too_many_arguments)]
fn validate_links(
    source: &str,
    file: &std::path::Path,
    links: &[scan::ScannedLink],
    validate_config: &crate::config::ValidateConfig,
    all_pages: &[CompiledPage],
    report: &Arc<RwLock<ValidationReport>>,
    root: &std::path::Path,
    nested_assets: &[(String, PathBuf)],
    flatten_outputs: &[String],
) {
    // Find the page for this file (needed for ResolveContext)
    let page = all_pages.iter().find(|p| p.route.source == *file);

    for link in links {
        // Determine if this is an asset attribute (src, poster, data, Image)
        let is_asset_attr = matches!(
            link.attr.as_str(),
            "src" | "poster" | "data" | "Src" | "Poster" | "Data" | "Image"
        );

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
                        let suggestion = nested_assets.iter().find_map(|(output_name, abs_source)| {
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
                    attr: &link.attr,
                };

                let space = GLOBAL_ADDRESS_SPACE.read();
                handle_resolve_result(
                    space.resolve(&link.dest, &ctx),
                    source,
                    &link.dest,
                    is_asset_attr,
                    validate_config,
                    report,
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
                    attr: &link.attr,
                };

                let space = GLOBAL_ADDRESS_SPACE.read();
                handle_resolve_result(
                    space.resolve(&link.dest, &ctx),
                    source,
                    &link.dest,
                    is_asset_attr,
                    validate_config,
                    report,
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
) {
    match result {
        ResolveResult::Found(_) | ResolveResult::External(_) => {}

        ResolveResult::NotFound { .. } => {
            if is_asset_attr {
                if validate_config.assets.enable {
                    report.write().add_asset(
                        source.to_string(),
                        link.to_string(),
                        "not found".to_string(),
                    );
                }
            } else if validate_config.pages.enable {
                report.write().add_page(
                    source.to_string(),
                    link.to_string(),
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
                report
                    .write()
                    .add_page(source.to_string(), link.to_string(), msg);
            }
        }

        ResolveResult::Warning { message, .. } => {
            if validate_config.pages.enable {
                report
                    .write()
                    .add_page(source.to_string(), link.to_string(), message);
            }
        }

        ResolveResult::Error { message } => {
            if validate_config.pages.enable {
                report
                    .write()
                    .add_page(source.to_string(), link.to_string(), message);
            }
        }
    }
}

/// Build AddressSpace using fast batch scanning (no full compilation)
/// Returns (pages, typst_links, compile_errors)
/// - pages: All compiled pages
/// - typst_links: HashMap<PathBuf, Vec<ScannedLink>> for Typst files
/// - compile_errors: Vec<(source_path, error_message)> for compile failures
fn build_address_space(
    config: &SiteConfig,
) -> Result<(
    Vec<CompiledPage>,
    HashMap<PathBuf, Vec<scan::ScannedLink>>,
    Vec<(String, String)>,
)> {
    use rayon::prelude::*;

    use crate::cli::common::scan_markdown_file;
    use crate::page::PageMeta;

    let root = crate::utils::path::normalize_path(config.get_root());
    let content_files = crate::compiler::collect_all_files(&config.build.content);
    let label = &config.build.meta.label;

    // Separate Typst and Markdown files
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    // Batch scan Typst files for metadata AND links (unified scan)
    let (typst_metas, typst_links_vec, compile_errors): (
        Vec<Option<PageMeta>>,
        Vec<Vec<scan::ScannedLink>>,
        Vec<(String, String)>,
    ) = if typst_files.is_empty() {
        (vec![], vec![], vec![])
    } else {
        match batch_scan_typst_unified(&typst_files, &root, label) {
            Ok((metas, links, errors)) => {
                let parsed_metas = metas
                    .into_iter()
                    .map(|json| json.and_then(|j| serde_json::from_value(j).ok()))
                    .collect();
                (parsed_metas, links, errors)
            }
            Err(e) => {
                // Fatal error (not per-file), bail
                return Err(e);
            }
        }
    };

    // Build CompiledPage for Typst files and collect links
    let mut pages: Vec<CompiledPage> = Vec::with_capacity(typst_files.len() + markdown_files.len());
    let mut typst_links: HashMap<PathBuf, Vec<scan::ScannedLink>> =
        HashMap::with_capacity(typst_files.len());

    for ((file, meta), links) in typst_files.iter().zip(typst_metas).zip(typst_links_vec) {
        if let Ok(mut page) = CompiledPage::from_paths(file, config) {
            page.content_meta = meta;
            page.apply_custom_permalink(config);
            typst_links.insert(page.route.source.clone(), links);
            pages.push(page);
        }
    }

    // Process Markdown files in parallel
    let markdown_pages: Vec<CompiledPage> = markdown_files
        .par_iter()
        .filter_map(|file| {
            let mut page = CompiledPage::from_paths(*file, config).ok()?;
            if let Ok(result) = scan_markdown_file(file, config) {
                page.content_meta = result
                    .raw_meta
                    .and_then(|json| serde_json::from_value(json).ok());
                page.apply_custom_permalink(config);
            }
            Some(page)
        })
        .collect();

    pages.extend(markdown_pages);

    // Build the global address space
    crate::compiler::page::build_address_space(&pages, config);

    Ok((pages, typst_links, compile_errors))
}

/// Batch scan Typst files for metadata AND links (unified scan)
/// Returns (metas, links, errors) where:
/// - metas: Vec<Option<JsonValue>> for each file
/// - links: Vec<Vec<ScannedLink>> for each file
/// - errors: Vec<(source_path, error_message)> for compile failures
fn batch_scan_typst_unified(
    files: &[&PathBuf],
    root: &std::path::Path,
    label: &str,
) -> Result<(
    Vec<Option<serde_json::Value>>,
    Vec<Vec<scan::ScannedLink>>,
    Vec<(String, String)>,
)> {
    use typst_batch::prelude::*;

    if files.is_empty() {
        return Ok((vec![], vec![], vec![]));
    }

    // Inject format="html" so image show rules output <img> tags for link extraction
    let scanner = Batcher::for_scan(root)
        .with_inputs([("format", "html")])
        .with_snapshot_from(files)?;

    match scanner.batch_scan(files) {
        Ok(results) => {
            let mut metas = Vec::with_capacity(results.len());
            let mut all_links = Vec::with_capacity(results.len());
            let mut errors = Vec::new();

            for (result, file) in results.into_iter().zip(files) {
                let rel_path = file
                    .strip_prefix(root)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string();

                match result {
                    Ok(scan) => {
                        metas.push(scan.metadata(label));
                        // Extract links and convert to ScannedLink
                        let links: Vec<scan::ScannedLink> = scan
                            .links()
                            .into_iter()
                            .map(|l| scan::ScannedLink {
                                dest: l.dest,
                                attr: format!("{:?}", l.source),
                            })
                            .collect();
                        all_links.push(links);
                    }
                    Err(e) => {
                        // Collect error instead of failing
                        errors.push((rel_path, e.to_string()));
                        metas.push(None);
                        all_links.push(vec![]);
                    }
                }
            }
            Ok((metas, all_links, errors))
        }
        Err(e) => {
            anyhow::bail!("Batch scan failed: {}", e);
        }
    }
}

/// Extract asset path from compile error.
/// "file not found (searched at /abs/root/images/photo.webp)" -> "/images/photo.webp"
fn extract_asset_path(error: &str, root: &std::path::Path) -> String {
    // Find "searched at " and extract the path
    if let Some(start) = error.find("searched at ") {
        let path_start = start + "searched at ".len();
        let path_end = error[path_start..].find(')').map(|e| path_start + e).unwrap_or(error.len());
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
