//! Query command implementation.
//!
//! Extracts metadata from content files in batch using parallel processing.
//! Uses fast scanning for Typst files (5-20x faster) and shared VDOM pipeline for Markdown.

mod collect;
mod output;
mod types;

use anyhow::Result;

use crate::cli::args::QueryArgs;
use crate::config::SiteConfig;
use crate::log;
use crate::utils::plural_count;

/// Execute query command
pub fn run_query(args: &QueryArgs, config: &SiteConfig) -> Result<()> {
    // Register VFS with nested asset mappings (no font warmup needed)
    let nested_mappings =
        crate::compiler::page::typst::init::build_nested_mappings(&config.build.assets.nested);
    crate::compiler::page::typst::init::init_vfs_with_mappings(
        config.get_root().to_path_buf(),
        nested_mappings,
    );

    // Populate STORED_PAGES with all site pages first
    // This ensures pages() returns correct data for all pages
    crate::cli::common::populate_stored_pages(config)?;

    let files = crate::cli::common::collect_content_files(&args.paths, &config.build.content)?;

    let file_count = files.len();
    log!("query"; "querying {}", plural_count(file_count, "file"));

    let results = collect::query_files(&files, args, config)?;

    log!(
        "query";
        "found {}",
        plural_count(results.pages.len(), "page with metadata")
    );

    output::output_results(&results, args)?;
    Ok(())
}
