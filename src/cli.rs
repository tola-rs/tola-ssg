//! Command-line interface definitions.
//!
//! Defines all CLI arguments and subcommands using clap.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Tola static site generator CLI
#[derive(Parser, Debug, Clone)]
#[command(version, about, long_about = None, arg_required_else_help = true)]
pub struct Cli {
    /// Output directory path (relative to project root)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Content directory path (relative to project root)
    #[arg(short, long)]
    pub content: Option<PathBuf>,

    /// Assets directory path (relative to project root)
    #[arg(short, long)]
    pub assets: Option<PathBuf>,

    /// Config file name (default: tola.toml)
    #[arg(short = 'C', long, default_value = "tola.toml")]
    pub config: PathBuf,

    /// subcommands
    #[command(subcommand)]
    pub command: Commands,
}

/// Shared build arguments for Build and Serve commands
#[derive(clap::Args, Debug, Clone)]
pub struct BuildArgs {
    /// Clean output directory completely before building
    #[arg(long)]
    pub clean: bool,

    /// Minify the html content
    #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub minify: Option<bool>,

    /// enable tailwindcss support
    #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub tailwind: Option<bool>,

    /// enable rss feed generation
    #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub rss: Option<bool>,

    /// enable sitemap generation
    #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub sitemap: Option<bool>,

    /// Override base URL for the site.
    ///
    /// Useful for CI/CD deployments where the production URL differs from local development.
    /// This avoids modifying tola.toml, keeping the source file clean.
    ///
    /// Example: Deploying to GitHub Pages project site (tola-ssg.github.io/example-sites/starter):
    ///   tola build --base-url "https://tola-ssg.github.io/example-sites/starter"
    #[arg(long = "base-url")]
    pub base_url: Option<String>,
}

/// Available subcommands
#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Init a template site
    Init {
        /// the name(path) of site directory, related to `root`
        name: Option<PathBuf>,
    },

    /// Deletes the output directory if there is one and rebuilds the site
    Build {
        #[command(flatten)]
        build_args: BuildArgs,
    },

    /// Serve the site. Rebuild and reload on change automatically
    Serve {
        #[command(flatten)]
        build_args: BuildArgs,

        /// Interface to bind on
        #[arg(short, long)]
        interface: Option<String>,

        /// The port you should provide
        #[arg(short, long)]
        port: Option<u16>,

        /// enable watch
        #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
        watch: Option<bool>,
    },

    /// Deletes the output directory if there is one and rebuilds the site
    Deploy {
        /// enable watch
        #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
        force: Option<bool>,
    },
}

#[allow(unused)]
impl Cli {
    pub const fn is_init(&self) -> bool {
        matches!(self.command, Commands::Init { .. })
    }
    pub const fn is_build(&self) -> bool {
        matches!(self.command, Commands::Build { .. })
    }
    pub const fn is_serve(&self) -> bool {
        matches!(self.command, Commands::Serve { .. })
    }
    pub const fn is_deploy(&self) -> bool {
        matches!(self.command, Commands::Deploy { .. })
    }
}
