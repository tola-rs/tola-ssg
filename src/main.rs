//! Tola - A static site generator for Typst blogs.

mod build;
mod cli;
mod config;
mod deploy;
mod init;
mod serve;
mod utils;
mod watch;

use anyhow::{Result, bail};
use build::build_site;
use clap::Parser;
use cli::{Cli, Commands};
use config::SiteConfig;
use deploy::deploy_site;
use gix::ThreadSafeRepository;
use init::new_site;
use serve::serve_site;
use std::path::Path;
use utils::rss::build_rss;

fn main() -> Result<()> {
    let cli: &'static Cli = Box::leak(Box::new(Cli::parse()));
    let config: &'static SiteConfig = Box::leak(Box::new(load_config(cli)?));

    match cli.command {
        Commands::Init { .. } => new_site(config),
        Commands::Build { .. } => build_all(config, true).map(|_| ()),
        Commands::Deploy { .. } => {
            let repo = build_all(config, true)?;
            deploy_site(repo, config)
        }
        Commands::Serve { .. } => {
            // Skip RSS generation during serve - it's not needed for local preview
            build_all(config, false)?;
            tokio::runtime::Runtime::new()?.block_on(serve_site(config))
        }
    }
}

/// Load and validate configuration from CLI arguments
fn load_config(cli: &'static Cli) -> Result<SiteConfig> {
    let root = cli.root.as_deref().unwrap_or(Path::new("./"));
    let config_path = root.join(&cli.config);

    let mut config = if config_path.exists() {
        SiteConfig::from_path(&config_path)?
    } else {
        SiteConfig::default()
    };
    config.update_with_cli(cli);

    // Validate config state based on command
    let config_exists = config.config_path.exists();
    match (cli.is_init(), config_exists) {
        (true, true) => {
            bail!("Config file already exists. Remove it manually or init in a different path.")
        }
        (false, false) => bail!("Config file not found."),
        _ => {}
    }

    if !cli.is_init() {
        config.validate()?;
    }

    Ok(config)
}

/// Build site and optionally generate RSS feed in parallel.
///
/// RSS generation is controlled by two factors:
/// - `with_rss`: caller decides if RSS should be considered (false for serve mode)
/// - `config.build.rss.enable`: user config to enable/disable RSS entirely
///
/// Both must be true for RSS to be generated.
fn build_all(config: &'static SiteConfig, with_rss: bool) -> Result<ThreadSafeRepository> {
    let (build_result, rss_result) = rayon::join(
        || build_site(config, config.build.clear),
        || {
            if with_rss { build_rss(config) } else { Ok(()) }
        },
    );

    rss_result?;
    build_result
}
