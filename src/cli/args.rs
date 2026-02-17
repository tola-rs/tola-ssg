//! Command-line interface definitions.

use clap::{ColorChoice, Parser, Subcommand};
use std::path::PathBuf;

/// Tola static site generator CLI
#[derive(Parser, Debug, Clone)]
#[command(version, about, long_about = None, arg_required_else_help = true)]
pub struct Cli {
    /// Control colored output (auto, always, never)
    #[arg(long, global = true, default_value = "auto")]
    pub color: ColorChoice,

    /// Output directory path (relative to project root)
    #[arg(short, long, value_hint = clap::ValueHint::DirPath)]
    pub output: Option<PathBuf>,

    /// Content directory path (relative to project root)
    #[arg(short, long, value_hint = clap::ValueHint::DirPath)]
    pub content: Option<PathBuf>,

    /// Config file path (default: tola.toml)
    #[arg(short = 'C', long, default_value = "tola.toml", value_hint = clap::ValueHint::FilePath)]
    pub config: PathBuf,

    /// subcommands
    #[command(subcommand)]
    pub command: Commands,
}

/// Available subcommands
#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Initialize a new site from template
    #[command(visible_alias = "i")]
    Init {
        /// Site directory name/path (relative to current directory)
        #[arg(value_hint = clap::ValueHint::DirPath)]
        name: Option<PathBuf>,
    },

    /// Build the site for production
    #[command(visible_alias = "b")]
    Build {
        #[command(flatten)]
        build_args: BuildArgs,
    },

    /// Start development server with hot reload
    #[command(visible_alias = "s")]
    Serve {
        #[command(flatten)]
        build_args: BuildArgs,

        /// Network interface to bind (e.g., 127.0.0.1, 0.0.0.0)
        #[arg(short, long)]
        interface: Option<std::net::IpAddr>,

        /// Port number to listen on
        #[arg(short, long)]
        port: Option<u16>,

        /// Enable file watching for auto-rebuild
        #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
        watch: Option<bool>,
    },

    /// Deploy the site to configured target
    #[command(visible_alias = "d")]
    Deploy {
        /// Force deploy even if there are uncommitted changes
        #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
        force: Option<bool>,
    },

    /// Query metadata from content files
    #[command(visible_alias = "q")]
    Query {
        #[command(flatten)]
        args: QueryArgs,
    },

    /// Validate site for broken links and missing assets
    #[command(visible_alias = "v")]
    Validate {
        #[command(flatten)]
        args: ValidateArgs,
    },
}

/// Validate command arguments.
#[derive(clap::Args, Debug, Clone)]
pub struct ValidateArgs {
    /// Files or directories to validate. If omitted, validates all content.
    /// Use `-` to read paths from stdin.
    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    /// Validate internal links (site pages)
    #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub internal: Option<bool>,

    /// Validate asset references
    #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub assets: Option<bool>,

    /// Treat validation failures as warnings instead of errors
    #[arg(long, short = 'w')]
    pub warn_only: bool,
}

/// Shared build arguments for Build and Serve commands
#[derive(clap::Args, Debug, Clone)]
pub struct BuildArgs {
    /// Clean output directory completely before building
    #[arg(short, long)]
    pub clean: bool,

    /// Minify the HTML content
    #[arg(short, long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub minify: Option<bool>,

    /// Enable CSS processor (e.g., TailwindCSS)
    #[arg(short = 'P', long = "css-processor", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub css_processor: Option<bool>,

    /// Enable RSS feed generation
    #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub rss: Option<bool>,

    /// Enable sitemap generation
    #[arg(short = 'S', long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
    pub sitemap: Option<bool>,

    /// Override site URL for deployment.
    ///
    /// Useful for CI/CD deployments where the production URL differs from local development.
    /// This avoids modifying tola.toml, keeping the source file clean.
    ///
    /// The path component is automatically extracted as `path_prefix` for subdirectory deployments.
    ///
    /// Example: Deploying to GitHub Pages project site (tola-ssg.github.io/example-sites/starter):
    ///   tola build --site-url "https://tola-ssg.github.io/example-sites/starter"
    #[arg(short = 'U', long = "site-url", value_hint = clap::ValueHint::Url)]
    pub site_url: Option<String>,

    /// Enable verbose output for debugging
    #[arg(short = 'V', long)]
    pub verbose: bool,

    /// Skip draft pages during build (default: false, drafts are included)
    #[arg(short = 'E', long)]
    pub skip_drafts: bool,
}

/// Query command arguments.
#[derive(clap::Args, Debug, Clone)]
pub struct QueryArgs {
    /// Paths to query (files, directories, or omit for all content).
    /// Use `-` to read paths from stdin (one per line).
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    pub paths: Vec<PathBuf>,

    /// Include draft pages in results
    #[arg(short, long)]
    pub drafts: bool,

    /// Output raw JSON without normalizing Typst content.
    /// Shows Typst elements as `{"func": "...", ...}` instead of HTML strings.
    #[arg(short, long)]
    pub raw: bool,

    /// Pretty-print JSON output
    #[arg(short, long)]
    pub pretty: bool,

    /// Filter out null/empty values from output
    #[arg(short = 'E', long)]
    pub filter_empty: bool,

    /// Filter output to specific fields (comma-separated)
    #[arg(short, long, value_delimiter = ',')]
    pub fields: Option<Vec<String>>,

    /// Write output to file instead of stdout
    #[arg(short, long, value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,
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
    pub const fn is_query(&self) -> bool {
        matches!(self.command, Commands::Query { .. })
    }
    pub const fn is_validate(&self) -> bool {
        matches!(self.command, Commands::Validate { .. })
    }
}
