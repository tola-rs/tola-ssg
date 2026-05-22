//! Tola - A static site generator for Typst blogs.

#![allow(dead_code)]

mod actor;
mod address;
mod asset;
mod cache;
mod cli;
mod compiler;
mod config;
mod core;
mod css;
mod embed;
mod freshness;
mod hooks;
mod image;
mod logger;
mod package;
mod page;
mod pipeline;
mod reload;
mod seo;
mod utils;

use address::SiteIndex;
use anyhow::Result;
use clap::{ColorChoice, Parser};
use cli::{Cli, Commands, build::build_site};
use config::{SiteConfig, init_config};
use core::BuildMode;

fn main() -> Result<()> {
    // Setup global Ctrl+C handler (before any blocking operations)
    core::setup_shutdown_handler()?;

    let cli: &'static Cli = Box::leak(Box::new(Cli::parse()));

    // Set global color override based on CLI option
    match cli.color {
        ColorChoice::Always => {
            owo_colors::set_override(true);
            logger::set_color_mode(logger::ColorMode::Always);
        }
        ColorChoice::Never => {
            owo_colors::set_override(false);
            logger::set_color_mode(logger::ColorMode::Never);
        }
        ColorChoice::Auto => {
            owo_colors::unset_override();
            logger::set_color_mode(logger::ColorMode::Auto);
        }
    }

    let config = init_config(SiteConfig::load(cli)?);

    match &cli.command {
        Commands::Init { name, dry } => cli::init::new_site(&config, name.is_some(), *dry),
        Commands::Build { .. } => build_all(&config, BuildMode::PRODUCTION),
        Commands::Deploy { .. } => {
            build_all(&config, BuildMode::PRODUCTION)?;
            cli::deploy::deploy_site(&config)
        }
        Commands::Serve { .. } => cli::serve::serve_with_cache(&config),
        Commands::Query { args } => cli::query::run_query(args, &config),
        Commands::Validate { .. } => cli::validate::validate_site(&config),
        Commands::Fix => cli::fix::run_fix(&config),
    }
}

/// Build site and optionally generate feed/sitemap outputs in parallel
fn build_all(config: &SiteConfig, mode: BuildMode) -> Result<()> {
    let state = SiteIndex::new();
    let _pages = build_site(mode, config, &state, false)?;

    // OG tags are injected during VDOM pipeline (see HeaderInjector).
    seo::build_outputs(config, &state)
}
