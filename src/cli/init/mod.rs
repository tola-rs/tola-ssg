//! Site initialization module.
//!
//! Creates new site structure with default configuration.
//!
mod files;
mod prompt;
mod tree;
mod validate;

use crate::{config::SiteConfig, logger, package::generate_lsp_stubs};
use anyhow::Result;
use std::path::{Path, PathBuf};

use validate::InitMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedFormat {
    Rss,
    Atom,
    Json,
}

impl FeedFormat {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::Atom => "atom",
            Self::Json => "json",
        }
    }

    const fn url(self) -> &'static str {
        match self {
            Self::Rss => "/feed.xml",
            Self::Atom => "/atom.xml",
            Self::Json => "/feed.json",
        }
    }
}

#[derive(Debug, Clone)]
struct Settings {
    title: String,
    base_url: Option<String>,
    language: String,
    author: String,
    email: String,
    atomic_css: bool,
    feeds: Vec<FeedFormat>,
    sitemap: bool,
}

impl Settings {
    fn recommended() -> Self {
        Self {
            title: String::new(),
            base_url: None,
            language: "en".into(),
            author: String::new(),
            email: String::new(),
            atomic_css: false,
            feeds: Vec::new(),
            sitemap: false,
        }
    }
}

/// Create a new site with default structure
///
/// # Steps
/// 1. Validate target directory
/// 2. Create directory structure
/// 3. Write configuration files
/// 4. Generate LSP stubs
///
/// If `dry_run` is true, only prints the config template to stdout.
pub fn new_site(
    site_config: &SiteConfig,
    has_name: bool,
    dry_run: bool,
    no_interactive: bool,
) -> Result<()> {
    if dry_run {
        logger::write_stdout(files::generate_config_template(&Settings::recommended()))?;
        return Ok(());
    }

    let root = site_config.get_root();
    let mode = if has_name {
        InitMode::NewDir
    } else {
        InitMode::CurrentDir
    };

    if let Err(e) = validate::validate_target(root, mode) {
        logger::prompt_log("error", format_args!("{}", e))?;
        std::process::exit(1);
    }

    let settings = collect_settings(root, no_interactive)?;

    tree::create_dirs(root)?;

    files::write_config(root, &settings)?;
    let output_dir = site_config.root_relative(&site_config.build.output);
    files::write_ignore_files(root, &output_dir)?;
    files::write_tola_lib(root)?;

    logger::blank();
    logger::log("init", format_args!("created site"));
    logger::blank();
    logger::text(&tree::render_tree(root));
    logger::blank();

    generate_lsp_stubs(root)?;

    logger::log("init", format_args!("generated Typst LSP stubs"));
    logger::log("init", format_args!("site initialized successfully"));
    Ok(())
}

fn collect_settings(root: &Path, no_interactive: bool) -> Result<Settings> {
    if !no_interactive && !prompt::can_prompt() {
        logger::prompt_log(
            "error",
            format_args!("interactive init requires a terminal"),
        )?;
        logger::prompt_line("hint: use `tola init --no-interactive` to generate the default site")?;
        std::process::exit(1);
    }

    logger::log("init", format_args!("create a new Tola site"));
    logger::log("init", format_args!("path: {}", display_path(root)));

    if no_interactive {
        logger::log("init", format_args!("using recommended settings"));
        return Ok(Settings::recommended());
    }

    logger::blank();
    if prompt::confirm("use recommended settings?", true)? {
        return Ok(Settings::recommended());
    }

    prompt::ask()
}

fn display_path(path: &Path) -> String {
    let normalized = crate::utils::path::normalize_path(path);
    if let Some(home) = std::env::var_os("HOME") {
        let home = crate::utils::path::normalize_path(&PathBuf::from(home));
        if let Ok(rest) = normalized.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".into();
            }
            return format!("~/{}", rest.display());
        }
    }
    normalized.display().to_string()
}
