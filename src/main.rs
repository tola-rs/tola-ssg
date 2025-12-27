//! Tola - A static site generator for Typst blogs.

mod build;
mod cli;
mod compiler;
mod config;
mod data;
mod deploy;
mod generator;
mod init;
mod logger;
mod serve;
mod typst_lib;
mod utils;
mod watch;

use anyhow::Result;
use build::build_site;
use clap::Parser;
use cli::{Cli, Commands};
use config::{cfg, init_config, SiteConfig};
use deploy::deploy_site;
use generator::{rss::build_rss, sitemap::build_sitemap};
use gix::ThreadSafeRepository;
use init::new_site;
use serve::serve_site;

fn main() -> Result<()> {
    let cli: &'static Cli = Box::leak(Box::new(Cli::parse()));
    init_config(SiteConfig::load(cli)?);

    match &cli.command {
        Commands::Init { name } => new_site(&cfg(), name.is_some()),
        Commands::Build { .. } => build_all().map(|_| ()),
        Commands::Deploy { .. } => {
            let repo = build_all()?;
            deploy_site(&repo, &cfg())
        }
        Commands::Serve { .. } => {
            build_all()?;
            serve_site()
        }
    }
}

/// Build site and optionally generate rss/sitemap in parallel.
///
/// rss generation is controlled by `config.build.rss.enable`.
/// Sitemap generation is controlled by `config.build.sitemap.enable`.
/// Output cleanup is controlled by `config.build.clean`.
fn build_all() -> Result<ThreadSafeRepository> {
    let c = cfg();
    // Build site first, collecting page metadata
    let (repo, pages) = build_site(&c)?;

    // Generate rss and sitemap in parallel using collected pages
    let (rss_result, sitemap_result) = rayon::join(
        || build_rss(&c, &pages),
        || build_sitemap(&c, &pages),
    );

    rss_result?;
    sitemap_result?;
    Ok(repo)
}

