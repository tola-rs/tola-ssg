//! Site configuration management for `tola.toml`.
//!
//! # Module Structure
//!
//! ```text
//! config/
//! ├── section/       # Configuration section definitions
//! │   ├── build/     # [build] and sub-sections
//! │   ├── deploy     # [deploy]
//! │   ├── serve      # [serve]
//! │   ├── site       # [site]
//! │   └── validate   # [validate]
//! ├── types/         # Utility types
//! │   ├── error      # ConfigError
//! │   ├── handle     # Global config handle
//! │   └── path       # PathResolver
//! └── mod.rs         # SiteConfig (this file)
//! ```
//!
//! # Sections
//!
//! | Section            | Purpose                                      |
//! |--------------------|----------------------------------------------|
//! | `[site.info]`      | Site metadata (title, author, url, extra)    |
//! | `[site.nav]`       | SPA navigation settings                      |
//! | `[site.preload]`   | Link prefetch settings                       |
//! | `[build]`          | Build paths, svg, css, feed, sitemap, etc.   |
//! | `[serve]`          | Development server (port, interface, watch)  |
//! | `[deploy]`         | Deployment targets (GitHub, Cloudflare)      |
//! | `[validate]`       | Link and asset validation settings           |

pub mod section;
pub mod types;
mod util;

use util::{extract_url_path, find_config_file};

// Re-export from section/
pub use section::{
    AssetsConfig, BuildSectionConfig, DeployConfig, FeedFormat, SlugCase, SlugConfig, SlugMode,
    SvgConverter, SvgFormat, ValidateConfig, ValidateLevel,
};

// Re-export from types/
pub use types::{
    ConfigDiagnostics, ConfigError, FieldPath, PathResolver, cfg, clear_clean_flag, init_config,
    reload_config,
};

// Internal imports from section/
use section::{ServeConfig, SiteSectionConfig, ThemeSectionConfig};

use crate::{
    cli::{BuildArgs, Cli, Commands, ValidateArgs},
    log,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

// ============================================================================
// root configuration
// ============================================================================

/// Root configuration structure representing tola.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteConfig {
    /// CLI arguments reference (internal use only)
    #[serde(skip)]
    pub cli: Option<&'static Cli>,

    /// Absolute path to the config file (internal use only)
    #[serde(skip)]
    pub config_path: PathBuf,

    /// Project root directory - parent of config file (internal use only)
    #[serde(skip)]
    pub root: PathBuf,

    /// Site configuration (info, nav, preload)
    #[serde(default)]
    pub site: SiteSectionConfig,

    /// Theme settings (recolor)
    #[serde(default)]
    pub theme: ThemeSectionConfig,

    /// Build settings
    #[serde(default)]
    pub build: BuildSectionConfig,

    /// Development server settings
    #[serde(default)]
    pub serve: ServeConfig,

    /// Deployment settings
    #[serde(default)]
    pub deploy: DeployConfig,

    /// Validation settings
    #[serde(default)]
    pub validate: ValidateConfig,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            cli: None,
            config_path: PathBuf::new(),
            root: PathBuf::new(),
            site: SiteSectionConfig::default(),
            theme: ThemeSectionConfig::default(),
            build: BuildSectionConfig::default(),
            serve: ServeConfig::default(),
            deploy: DeployConfig::default(),
            validate: ValidateConfig::default(),
        }
    }
}

impl SiteConfig {
    /// Load configuration from CLI arguments.
    ///
    /// For non-Init commands, searches upward from cwd to find config file.
    /// The project root is determined by the config file's parent directory.
    pub fn load(cli: &'static Cli) -> Result<Self> {
        let (config_path, exists) = Self::resolve_config_path(cli)?;

        // Validate config existence (skip for init)
        if !cli.is_init() && !exists {
            log!(
                "error";
                "Config file '{}' not found. Run 'tola init' to create a new project.",
                cli.config.display()
            );
            std::process::exit(1);
        }

        // Load or create default config
        let mut config = if exists && !cli.is_init() {
            Self::from_path(&config_path)?
        } else {
            Self::default()
        };

        // Validate raw paths before normalization
        if !cli.is_init() {
            config.validate_paths()?;
        }

        // Set paths and apply CLI options
        config.config_path = config_path;
        config.cli = Some(cli);
        config.finalize(cli);

        // Full validation (skip for init: no config file yet)
        if !cli.is_init() {
            config.validate()?;
            // Filter out non-existent deps after validation warning
            config.build.filter_existing_deps();
        }

        Ok(config)
    }

    /// Resolve config file path based on command.
    fn resolve_config_path(cli: &Cli) -> Result<(PathBuf, bool)> {
        let cwd = std::env::current_dir().context("Failed to get current working directory")?;

        match &cli.command {
            Commands::Init { name: Some(name) } => {
                let path = cwd.join(name).join(&cli.config);
                let exists = path.exists();
                Ok((path, exists))
            }
            Commands::Init { name: None } => {
                let path = cwd.join(&cli.config);
                let exists = path.exists();
                Ok((path, exists))
            }
            _ => {
                // Search upward from cwd
                match find_config_file(&cli.config) {
                    Some(path) => Ok((path, true)),
                    None => Ok((cwd.join(&cli.config), false)),
                }
            }
        }
    }

    /// Finalize configuration after loading.
    fn finalize(&mut self, cli: &Cli) {
        // Resolve root path
        let root = match &cli.command {
            Commands::Init { name: Some(name) } => {
                std::env::current_dir().unwrap_or_default().join(name)
            }
            Commands::Init { name: None } => std::env::current_dir().unwrap_or_default(),
            _ => self
                .config_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        };

        self.set_root(&root);
        self.normalize_paths(&root);
        self.apply_command_options(cli);

        // Extract path_prefix from site.url
        // This ensures path_prefix works for both:
        // - CLI: --site-url "https://example.github.io/my-project"
        // - Config: [site.info] url = "https://example.github.io/my-project"
        self.sync_path_prefix_from_url();

        // In serve mode, clear path_prefix unless respect_prefix is enabled
        // This allows local development to access pages at / instead of /prefix/
        if matches!(cli.command, Commands::Serve { .. }) && !self.serve.respect_prefix {
            self.build.path_prefix = PathBuf::new();
        }
    }

    /// Derive path_prefix from site.info.url.
    ///
    /// This extracts the URL path component and sets it as path_prefix,
    /// enabling proper link generation for subdirectory deployments
    /// (e.g., GitHub Pages project sites).
    fn sync_path_prefix_from_url(&mut self) {
        // Extract path from site.url
        if let Some(ref url) = self.site.info.url
            && let Some(path) = extract_url_path(url)
            && !path.is_empty()
        {
            self.build.path_prefix = PathBuf::from(path);
        }
    }

    /// Parse configuration from TOML string
    pub fn from_str(content: &str) -> Result<Self> {
        let config: Self = toml::from_str(content)?;
        Ok(config)
    }

    /// Load configuration from file path with unknown field detection.
    fn from_path(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).map_err(|err| ConfigError::Io(path.to_path_buf(), err))?;

        let (config, ignored) = Self::parse_with_ignored(&content)?;

        if !ignored.is_empty() {
            Self::print_unknown_fields_warning(&ignored, path);
            if !Self::prompt_continue()? {
                bail!("Aborted due to unknown config fields");
            }
        }

        Ok(config)
    }

    /// Parse TOML content, collecting any unknown fields.
    fn parse_with_ignored(content: &str) -> Result<(Self, Vec<String>)> {
        let mut ignored = Vec::new();
        let deserializer = toml::Deserializer::new(content);
        let config = serde_ignored::deserialize(deserializer, |path: serde_ignored::Path| {
            ignored.push(path.to_string());
        })?;
        Ok((config, ignored))
    }

    /// Print warning about unknown fields.
    fn print_unknown_fields_warning(fields: &[String], path: &Path) {
        // Show only filename (tola.toml) since it's always at site root
        let display_path = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_else(|| path.to_string_lossy());
        eprintln!();
        log!("warning"; "unknown fields in {}:", display_path);
        log!("warning"; "ignoring:");
        for field in fields {
            eprintln!("- {}", field);
        }
        eprintln!();
    }

    /// Prompt user to continue. Returns true only if user explicitly confirms.
    fn prompt_continue() -> Result<bool> {
        use std::io::{self, Write};

        eprint!("Continue? [y/N] ");
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let input = input.trim().to_lowercase();
        // Default no (empty input), explicit "y" or "yes" to continue
        Ok(input == "y" || input == "yes")
    }

    /// Get the root directory path
    pub fn get_root(&self) -> &Path {
        &self.root
    }

    /// Set the root directory path
    pub fn set_root(&mut self, path: &Path) {
        self.root = path.to_path_buf();
    }

    /// Join a path with the root directory.
    ///
    /// Shorthand for `config.get_root().join(path)`.
    pub fn root_join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.root.join(path)
    }

    /// Get path relative to the site root
    pub fn root_relative(&self, path: impl AsRef<Path>) -> PathBuf {
        path.as_ref()
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.as_ref().to_path_buf())
    }

    /// Get CLI arguments reference
    pub const fn get_cli(&self) -> &'static Cli {
        self.cli.unwrap()
    }

    /// Get path resolver for consistent path/URL generation.
    ///
    /// This is the single source of truth for all path operations,
    /// eliminating manual `path_prefix` handling throughout the codebase.
    ///
    /// # Example
    /// ```ignore
    /// let paths = config.paths();
    /// let output_dir = paths.output_dir();
    /// let url = paths.url_for_filename("styles.css");
    /// ```
    pub fn paths(&self) -> PathResolver<'_> {
        PathResolver::new(&self.build.output, &self.build.path_prefix)
    }

    // ========================================================================
    // cli configuration updates
    // ========================================================================

    /// Apply command-specific configuration options.
    fn apply_command_options(&mut self, cli: &Cli) {
        match &cli.command {
            Commands::Build { build_args } => {
                self.apply_build_args(build_args, false);
            }
            Commands::Serve {
                build_args,
                interface,
                port,
                watch,
                ..
            } => {
                self.apply_build_args(build_args, true);
                self.apply_serve_options(*interface, *port, *watch);
            }
            Commands::Deploy { force } => {
                Self::update_option(&mut self.deploy.force, force.as_ref());
            }
            Commands::Init { .. } => {}
            // Query command doesn't modify config
            Commands::Query { .. } => {}
            // Validate command: CLI args override config
            Commands::Validate { args } => {
                self.apply_validate_args(args);
            }
        }
    }

    /// Apply validate arguments from CLI.
    fn apply_validate_args(&mut self, args: &ValidateArgs) {
        // CLI flags override config enable settings
        Self::update_option(
            &mut self.validate.link.internal.enable,
            args.internal.as_ref(),
        );
        Self::update_option(&mut self.validate.assets.enable, args.assets.as_ref());

        // --warn-only sets all levels to Warn
        if args.warn_only {
            self.validate.link.internal.level = ValidateLevel::Warn;
            self.validate.assets.level = ValidateLevel::Warn;
        }
    }

    /// Apply build arguments from CLI.
    ///
    /// `is_serve`: If true, rss/sitemap default to disabled for faster local preview.
    fn apply_build_args(&mut self, args: &BuildArgs, is_serve: bool) {
        // Set verbose mode globally
        crate::logger::set_verbose(args.verbose);

        Self::update_option(&mut self.build.minify, args.minify.as_ref());
        Self::update_option(
            &mut self.build.css.processor.enable,
            args.css_processor.as_ref(),
        );
        self.build.clean = args.clean;
        self.build.skip_drafts = args.skip_drafts;

        // Override site URL if provided via CLI
        // path_prefix will be derived from it in sync_path_prefix_from_url()
        if let Some(ref url) = args.site_url {
            self.site.info.url = Some(url.clone());
        }

        if is_serve {
            // Serve: disable feed/sitemap by default, enable only if explicitly requested
            self.site.feed.enable = args.rss.unwrap_or(false);
            self.site.sitemap.enable = args.sitemap.unwrap_or(false);
        } else {
            // Build/Deploy: respect config, override only if CLI flag provided
            Self::update_option(&mut self.site.feed.enable, args.rss.as_ref());
            Self::update_option(&mut self.site.sitemap.enable, args.sitemap.as_ref());
        }
    }

    /// Apply serve-specific options.
    fn apply_serve_options(
        &mut self,
        interface: Option<std::net::IpAddr>,
        port: Option<u16>,
        watch: Option<bool>,
    ) {
        Self::update_option(&mut self.serve.interface, interface.as_ref());
        Self::update_option(&mut self.serve.port, port.as_ref());
        Self::update_option(&mut self.serve.watch, watch.as_ref());

        // Set base URL for local development (only if not overridden via CLI --base-url)
        if self.site.info.url.is_none() {
            self.site.info.url = Some(format!(
                "http://{}:{}",
                self.serve.interface, self.serve.port
            ));
        }
    }

    /// Update config option if CLI value is provided.
    fn update_option<T: Clone>(config_option: &mut T, cli_option: Option<&T>) {
        if let Some(option) = cli_option {
            *config_option = option.clone();
        }
    }

    // ========================================================================
    // path normalization
    // ========================================================================

    /// Normalize all paths relative to root directory.
    fn normalize_paths(&mut self, root: &Path) {
        let cli = self.get_cli();

        // Apply CLI path overrides first
        Self::update_option(&mut self.build.content, cli.content.as_ref());
        Self::update_option(&mut self.build.output, cli.output.as_ref());

        // Normalize root to absolute path
        let root = crate::utils::path::normalize_path(root);
        self.set_root(&root);

        // Normalize config path (already set in main.rs, just canonicalize)
        self.config_path = crate::utils::path::normalize_path(&self.config_path);

        // Normalize build directories
        self.build.content = crate::utils::path::normalize_path(&root.join(&self.build.content));
        // Normalize assets paths
        self.build.assets.normalize(&root);
        self.build.output = crate::utils::path::normalize_path(&root.join(&self.build.output));
        self.build.deps = self
            .build
            .deps
            .iter()
            .map(|p| crate::utils::path::normalize_path(&root.join(p)))
            .collect();
        // Note: feed.path and sitemap.path are kept as relative filenames.
        // They are resolved to output_dir() at write time to include path_prefix.

        // Normalize optional paths
        self.normalize_optional_paths(&root);
    }

    /// Normalize optional paths (CSS processor input, deploy token).
    fn normalize_optional_paths(&mut self, root: &Path) {
        if let Some(input) = self.build.css.processor.input.take() {
            self.build.css.processor.input =
                Some(crate::utils::path::normalize_path(&root.join(input)));
        }

        if let Some(token_path) = self.deploy.github.token_path.take() {
            self.deploy.github.token_path = Some(Self::normalize_token_path(&token_path, root));
        }
    }

    /// Normalize token path with tilde expansion.
    fn normalize_token_path(path: &Path, root: &Path) -> PathBuf {
        let expanded = shellexpand::tilde(path.to_str().unwrap_or_default()).into_owned();
        let path = PathBuf::from(expanded);
        let full_path = if path.is_relative() {
            root.join(&path)
        } else {
            path
        };
        crate::utils::path::normalize_path(&full_path)
    }

    // ========================================================================
    // validation
    // ========================================================================

    /// Pre-validate paths before normalization.
    ///
    /// This must be called before `finalize()` because path normalization
    /// converts relative paths to absolute paths, making it impossible to
    /// detect if the user specified an absolute path in the config.
    fn validate_paths(&self) -> Result<()> {
        let mut diag = ConfigDiagnostics::new();

        // Validate assets paths (must be relative)
        self.build.assets.validate_paths(&mut diag);

        diag.into_result()
            .map_err(|e| ConfigError::Diagnostics(e).into())
    }

    /// Validate configuration for the current command.
    ///
    /// Collects all validation errors and returns them at once.
    pub fn validate(&self) -> Result<()> {
        let mut diag = ConfigDiagnostics::with_allow_experimental(self.build.allow_experimental);

        if !self.config_path.exists() {
            bail!(ConfigError::Validation("config file not found".into()));
        }

        // Validate field status (experimental, deprecated, not_implemented)
        self.site.validate_field_status(&mut diag);
        self.deploy.validate_field_status(&mut diag);
        self.build.svg.validate_field_status(&mut diag);

        // Validate each section
        self.site.info.validate(self.site.feed.enable, &mut diag);
        self.build.validate(&mut diag);
        self.build.css.validate(&mut diag);
        self.build.svg.validate(&mut diag);
        self.build.assets.validate(&mut diag);
        self.site
            .header
            .validate(&self.build.assets, self.get_root(), &mut diag);

        // Command-specific validation
        self.validate_command_specific(&mut diag)?;

        // Print collected hints and warnings (grouped display)
        diag.print_hints_and_warnings();

        // Return all collected errors
        diag.into_result()
            .map_err(|e| ConfigError::Diagnostics(e).into())
    }

    /// Validate command-specific requirements.
    fn validate_command_specific(&self, diag: &mut ConfigDiagnostics) -> Result<()> {
        if let Commands::Deploy { .. } = &self.get_cli().command {
            self.deploy.validate(diag);
        }
        Ok(())
    }
}

// ============================================================================
// Test Helpers (available to all modules via `use crate::config::test_*`)
// ============================================================================

/// Parse config with minimal required `[site.info]` fields.
/// Panics if there are unknown fields (to catch config typos in tests).
#[cfg(test)]
pub fn test_parse_config(extra: &str) -> SiteConfig {
    let config = format!("[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n{extra}");
    let (parsed, ignored) = SiteConfig::parse_with_ignored(&config).unwrap();
    assert!(
        ignored.is_empty(),
        "test config has unknown fields: {:?}",
        ignored
    );
    parsed
}

// ============================================================================
// tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_str_invalid_toml() {
        // Invalid TOML syntax - unclosed bracket
        let result: Result<SiteConfig, _> = toml::from_str("[base\ntitle = \"My Blog\"");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_root_default() {
        let config = SiteConfig::default();
        // Default root is empty PathBuf, set during config loading
        assert_eq!(config.get_root(), Path::new(""));
    }

    #[test]
    fn test_set_root() {
        let mut config = SiteConfig::default();
        config.set_root(Path::new("/custom/path"));
        assert_eq!(config.get_root(), Path::new("/custom/path"));
    }

    #[test]
    fn test_site_config_default() {
        let config = SiteConfig::default();

        assert!(config.cli.is_none());
        assert_eq!(config.config_path, PathBuf::new());
        assert_eq!(config.site.info.title, "");
        assert!(config.build.minify);
        assert_eq!(config.serve.port, 5277);
        assert_eq!(config.deploy.provider, "github");
    }

    #[test]
    fn test_unknown_fields_detected() {
        let content = "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n[unknown_section]\nfield = \"value\"";
        let (config, ignored) = SiteConfig::parse_with_ignored(content).unwrap();

        // Config should parse successfully
        assert_eq!(config.site.info.title, "Test");

        // Unknown fields should be collected
        assert!(!ignored.is_empty());
        assert!(ignored.iter().any(|f| f.contains("unknown_section")));
    }

    #[test]
    fn test_no_unknown_fields() {
        let content = "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"";
        let (_, ignored) = SiteConfig::parse_with_ignored(content).unwrap();
        assert!(ignored.is_empty());
    }
}
