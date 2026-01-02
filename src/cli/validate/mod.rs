//! Site validation command.

mod report;
mod scan;

use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;
use rayon::prelude::*;

use super::common::{batch_scan_typst_metadata, collect_content_files};
use crate::compiler::page::CompiledPage;
use crate::config::SiteConfig;
use crate::core::{ContentKind, GLOBAL_ADDRESS_SPACE, LinkKind, ResolveContext, ResolveResult};
use crate::log;
use crate::utils::{plural_count, plural_s};

use report::ValidationReport;
use scan::{ScanResult, scan_markdown, scan_typst_batch};

/// Validate site links and assets.
pub fn validate_site(config: &SiteConfig) -> Result<()> {
    // Register VFS for @tola/* virtual packages (no font warmup needed)
    crate::compiler::page::typst::init::init_vfs();

    let args = get_validate_args();
    let files = collect_content_files(&args.paths, &config.build.content)?;

    if files.is_empty() {
        log!("validate"; "no content files found");
        return Ok(());
    }

    let file_count = files.len();
    let validate_config = &config.validate;

    // Check if any validation is enabled
    let check_internal = validate_config.link.internal.enable;
    let check_assets = validate_config.assets.enable;

    if !check_internal && !check_assets {
        log!("validate"; "no checks enabled");
        return Ok(());
    }

    log!("validate"; "validating {}", plural_count(file_count, "file"));

    // Setup paths
    let root = crate::utils::path::normalize_path(config.get_root());

    // Build AddressSpace for internal validation if needed
    let all_pages = if check_internal || check_assets {
        build_address_space(config)?
    } else {
        Vec::new()
    };

    // Unified report
    let report = Arc::new(RwLock::new(ValidationReport::default()));

    // Parallel scan all files (collects internal/asset errors)
    scan_all_files(&files, &root, config, &all_pages, &report);

    // Log internal link results
    if check_internal {
        let count = report.read().internal_error_count();
        if count > 0 {
            log!("validate"; "found {} broken internal link{}", count, plural_s(count));
        } else {
            log!("validate"; "all internal links valid");
        }
    }

    // Log asset results
    if check_assets {
        let count = report.read().asset_error_count();
        if count > 0 {
            log!("validate"; "found {} missing asset{}", count, plural_s(count));
        } else {
            log!("validate"; "all assets valid");
        }
    }

    // Get final report
    let report = Arc::try_unwrap(report).unwrap().into_inner();

    // Print detailed report (internal -> assets)
    report.print();

    // Final summary (internal -> assets)
    print_summary(report.internal_file_count(), report.asset_file_count())
}

/// Scan all content files in parallel, collecting validation data.
fn scan_all_files(
    files: &[std::path::PathBuf],
    root: &std::path::Path,
    config: &SiteConfig,
    all_pages: &[CompiledPage],
    report: &Arc<RwLock<ValidationReport>>,
) {
    let validate_config = &config.validate;
    // Collect all nested asset source directories with their output prefixes
    // e.g., ("images", "/absolute/path/to/assets/images") means /images/* maps to assets/images/*
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

    // Separate Typst and Markdown files
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(files);

    // Batch scan Typst files
    let typst_results = scan_typst_batch(&typst_files, root);

    // Process Typst results
    for (file, result) in typst_files.iter().zip(typst_results) {
        if let Some(result) = result {
            collect_scan_result(
                result,
                file,
                validate_config,
                all_pages,
                report,
                &nested_assets,
                &flatten_outputs,
            );
        }
    }

    // Process Markdown files in parallel
    markdown_files.par_iter().for_each(|file| {
        if let Ok(result) = scan_markdown(file, root, config) {
            collect_scan_result(
                result,
                file,
                validate_config,
                all_pages,
                report,
                &nested_assets,
                &flatten_outputs,
            );
        }
    });
}

/// Collect results from a single file scan.
#[allow(clippy::too_many_arguments)]
fn collect_scan_result(
    result: ScanResult,
    file: &std::path::Path,
    validate_config: &crate::config::ValidateConfig,
    all_pages: &[CompiledPage],
    report: &Arc<RwLock<ValidationReport>>,
    nested_assets: &[(String, std::path::PathBuf)],
    flatten_outputs: &[String],
) {
    // Find the page for this file (needed for ResolveContext)
    let page = all_pages.iter().find(|p| p.route.source == *file);

    for link in &result.links {
        // Determine if this is an asset attribute (src, poster, data)
        let is_asset_attr = matches!(
            link.attr.as_str(),
            "src" | "poster" | "data" | "Src" | "Poster" | "Data"
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
                    let in_nested = nested_assets.iter().any(|(output_name, source_dir)| {
                        // Exact match: /assets -> output_name "assets"
                        if trimmed == output_name {
                            return source_dir.exists();
                        }
                        // Prefix with slash: /assets/xxx -> output_name "assets", rest "xxx"
                        if let Some(rest) = trimmed.strip_prefix(output_name)
                            && let Some(rest) = rest.strip_prefix('/')
                        {
                            return source_dir.join(rest).exists();
                        }
                        false
                    });

                    // Check flatten outputs (e.g., /favicon.ico -> "favicon.ico")
                    let in_flatten = flatten_outputs.iter().any(|name| trimmed == name);

                    if in_nested || in_flatten {
                        continue;
                    }

                    // Asset not found - report error if assets validation enabled
                    if validate_config.assets.enable {
                        report.write().add_asset(
                            result.source.clone(),
                            link.dest.clone(),
                            "not found".to_string(),
                        );
                    }
                    continue;
                }

                // Try AddressSpace for non-asset links
                if !validate_config.link.internal.enable {
                    continue;
                }

                let Some(page) = page else { continue };

                let ctx = ResolveContext {
                    current_permalink: &page.route.permalink,
                    source_path: &page.route.source,
                    colocated_dir: page.route.colocated_dir.as_deref(),
                    attr: &link.attr,
                };

                let space = GLOBAL_ADDRESS_SPACE.read();
                handle_resolve_result(
                    space.resolve(&link.dest, &ctx),
                    &result.source,
                    &link.dest,
                    is_asset_attr,
                    validate_config,
                    report,
                );
            }

            // File-relative and fragment links: validate via AddressSpace
            LinkKind::FileRelative(_) | LinkKind::Fragment(_) => {
                if !validate_config.link.internal.enable && !validate_config.assets.enable {
                    continue;
                }

                let Some(page) = page else { continue };

                let ctx = ResolveContext {
                    current_permalink: &page.route.permalink,
                    source_path: &page.route.source,
                    colocated_dir: page.route.colocated_dir.as_deref(),
                    attr: &link.attr,
                };

                let space = GLOBAL_ADDRESS_SPACE.read();
                handle_resolve_result(
                    space.resolve(&link.dest, &ctx),
                    &result.source,
                    &link.dest,
                    is_asset_attr,
                    validate_config,
                    report,
                );
            }
        }
    }
}

/// Handle AddressSpace resolve result.
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
            } else if validate_config.link.internal.enable {
                report.write().add_internal(
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
            if validate_config.link.internal.enable {
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
                    .add_internal(source.to_string(), link.to_string(), msg);
            }
        }

        ResolveResult::Warning { message, .. } => {
            if validate_config.link.internal.enable {
                report
                    .write()
                    .add_internal(source.to_string(), link.to_string(), message);
            }
        }

        ResolveResult::Error { message } => {
            if validate_config.link.internal.enable {
                report
                    .write()
                    .add_internal(source.to_string(), link.to_string(), message);
            }
        }
    }
}

/// Build AddressSpace using fast batch scanning (no full compilation).
fn build_address_space(config: &SiteConfig) -> Result<Vec<CompiledPage>> {
    use rayon::prelude::*;

    use crate::cli::common::scan_markdown_file;
    use crate::page::PageMeta;

    let root = crate::utils::path::normalize_path(config.get_root());
    let content_files = crate::compiler::collect_all_files(&config.build.content);
    let label = &config.build.meta.label;

    // Separate Typst and Markdown files
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    // Batch scan Typst files for metadata
    let typst_metas: Vec<Option<PageMeta>> = if typst_files.is_empty() {
        vec![]
    } else {
        batch_scan_typst_metadata(&typst_files, &root, label)?
            .into_iter()
            .map(|json| json.and_then(|j| serde_json::from_value(j).ok()))
            .collect()
    };

    // Build CompiledPage for Typst files
    let mut pages: Vec<CompiledPage> = typst_files
        .iter()
        .zip(typst_metas)
        .filter_map(|(file, meta)| {
            let mut page = CompiledPage::from_paths(file, config).ok()?;
            page.content_meta = meta;
            page.apply_custom_permalink(config);
            Some(page)
        })
        .collect();

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

    Ok(pages)
}

/// Print final summary and return error if validation failed.
fn print_summary(internal_errors: usize, asset_errors: usize) -> Result<()> {
    if internal_errors > 0 || asset_errors > 0 {
        let mut parts = Vec::new();
        if internal_errors > 0 {
            parts.push(format!(
                "{} with internal link errors",
                plural_count(internal_errors, "file")
            ));
        }
        if asset_errors > 0 {
            parts.push(format!(
                "{} with asset errors",
                plural_count(asset_errors, "file")
            ));
        }
        anyhow::bail!("found {}", parts.join(", "));
    }

    log!("validate"; "all checks passed");
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
            internal: None,
            external: None,
            assets: None,
        },
    }
}
