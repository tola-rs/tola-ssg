//! Site configuration management for `tola.toml`.
//!
//! # Sections
//!
//! | Section     | Purpose                                      |
//! |-------------|----------------------------------------------|
//! | `[base]`    | Site metadata (title, author, url)           |
//! | `[build]`   | Build paths, typst, tailwind, rss, etc.      |
//! | `[serve]`   | Development server (port, interface, watch)  |
//! | `[deploy]`  | Deployment targets (GitHub, Cloudflare)      |
//! | `[extra]`   | User-defined custom fields                   |
//!
//! # Example
//!
//! ```toml
//! [base]
//! title = "My Blog"
//! description = "A personal blog"
//! url = "https://example.com"
//!
//! [build]
//! content = "content"
//! output = "public"
//! minify = true
//!
//! [build.rss]
//! enable = true
//!
//! [serve]
//! port = 5277
//!
//! [extra]
//! analytics_id = "UA-12345"
//! ```

mod base;
mod build;
pub mod defaults;
mod deploy;
mod error;
pub mod handle;
mod serve;

// Re-export public types used by other modules
pub use build::{BuildConfig, ExtractSvgType, SlugCase, SlugMode, SlugSeparator};
pub use deploy::DeployConfig;
pub use error::ConfigError;
pub use handle::{cfg, init_config, reload_config};

// Internal imports used in this module
use base::BaseConfig;
use serve::ServeConfig;

use crate::cli::{BuildArgs, Cli, Commands};
use anyhow::{Context, Result, bail};
use educe::Educe;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

// ============================================================================
// helper functions
// ============================================================================

/// Parse a human-readable size string into bytes.
///
/// Supports suffixes: B (bytes), KB (kilobytes), MB (megabytes).
/// Case-insensitive for the suffix.
///
/// # Examples
/// ```ignore
/// parse_size_string("20KB") // → 20480
/// parse_size_string("5MB")  // → 5242880
/// parse_size_string("100B") // → 100
/// parse_size_string("100")  // → 100 (defaults to bytes)
/// ```
fn parse_size_string(s: &str) -> usize {
    let s = s.to_uppercase();
    let (multiplier, suffix_len) = if s.ends_with("MB") {
        (1024 * 1024, 2)
    } else if s.ends_with("KB") {
        (1024, 2)
    } else if s.ends_with('B') {
        (1, 1)
    } else {
        (1, 0)
    };
    let value: usize = s[..s.len() - suffix_len].trim().parse().unwrap_or(0);
    multiplier * value
}

/// Find config file by searching upward from current directory.
///
/// Starts from cwd and walks up parent directories until finding `config_name`.
/// Returns the absolute path to the config file if found.
///
/// # Example
/// ```text
/// /home/user/site/content/posts/  ← cwd
/// /home/user/site/tola.toml       ← found!
/// ```
pub fn find_config_file(config_name: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;

    // First check if config_name is an absolute path or exists in cwd
    if config_name.is_absolute() && config_name.exists() {
        return Some(config_name.to_path_buf());
    }

    // Walk up from cwd looking for config file
    let mut current = cwd.as_path();
    loop {
        let candidate = current.join(config_name);
        if candidate.exists() {
            return Some(candidate);
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) => current = parent,
            None => return None, // Reached filesystem root
        }
    }
}

// ============================================================================
// root configuration
// ============================================================================

/// Root configuration structure representing tola.toml
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
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

    /// Basic site information
    #[serde(default)]
    pub base: BaseConfig,

    /// Build settings
    #[serde(default)]
    pub build: BuildConfig,

    /// Development server settings
    #[serde(default)]
    pub serve: ServeConfig,

    /// Deployment settings
    #[serde(default)]
    pub deploy: DeployConfig,

    /// User-defined extra fields
    #[serde(default)]
    pub extra: HashMap<String, toml::Value>,
}

impl SiteConfig {
    /// Load configuration from CLI arguments.
    ///
    /// For non-Init commands, searches upward from cwd to find config file.
    /// The project root is determined by the config file's parent directory.
    pub fn load(cli: &'static Cli) -> Result<Self> {
        let (config_path, exists) = Self::resolve_config_path(cli)?;

        // Validate config existence
        match (cli.is_init(), exists) {
            (true, true) => bail!("Config file already exists. Remove it manually or init in a different path."),
            (false, false) => bail!("Config file '{}' not found. Run 'tola init' to create a new project.", cli.config.display()),
            _ => {}
        }

        // Load or create default config
        let mut config = if exists {
            Self::from_path(&config_path)?
        } else {
            Self::default()
        };

        // Set paths and apply CLI options
        config.config_path = config_path;
        config.cli = Some(cli);
        config.finalize(cli);

        // Validate (skip for init)
        if !cli.is_init() {
            config.validate()?;
        }

        Ok(config)
    }

    /// Resolve config file path based on command.
    fn resolve_config_path(cli: &Cli) -> Result<(PathBuf, bool)> {
        let cwd = std::env::current_dir()?;

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
            Commands::Init { name: None } => {
                std::env::current_dir().unwrap_or_default()
            }
            _ => self.config_path.parent().map(Path::to_path_buf).unwrap_or_default(),
        };

        self.set_root(&root);
        self.normalize_paths(&root);
        self.apply_command_options(cli);
    }

    /// Parse configuration from TOML string
    pub fn from_str(content: &str) -> Result<Self> {
        let config: Self = toml::from_str(content)?;
        Ok(config)
    }

    /// Load configuration from file path
    fn from_path(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).map_err(|err| ConfigError::Io(path.to_path_buf(), err))?;
        Self::from_str(&content)
    }

    /// Get the root directory path
    pub fn get_root(&self) -> &Path {
        &self.root
    }

    /// Set the root directory path
    pub fn set_root(&mut self, path: &Path) {
        self.root = path.to_path_buf();
    }

    /// Get CLI arguments reference
    pub const fn get_cli(&self) -> &'static Cli {
        self.cli.unwrap()
    }

    /// Parse `inline_max_size` string to bytes.
    ///
    /// Supports suffixes: B (bytes), KB (kilobytes), MB (megabytes).
    ///
    /// # Examples
    /// - "20KB" → 20480
    /// - "5MB" → 5242880
    /// - "100B" → 100
    pub fn get_inline_max_size(&self) -> usize {
        parse_size_string(&self.build.typst.svg.inline_max_size)
    }

    /// Get DPI scale factor (relative to standard 96 DPI).
    ///
    /// Used for SVG rendering resolution calculation.
    pub fn get_scale(&self) -> f32 {
        self.build.typst.svg.dpi / 96.0
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
            Commands::Serve { build_args, interface, port, watch, .. } => {
                self.apply_build_args(build_args, true);
                self.apply_serve_options(interface.as_ref(), *port, *watch);
            }
            Commands::Deploy { force } => {
                Self::update_option(&mut self.deploy.force, force.as_ref());
            }
            Commands::Init { .. } => {}
        }
    }

    /// Apply build arguments from CLI.
    ///
    /// `is_serve`: If true, rss/sitemap default to disabled for faster local preview.
    fn apply_build_args(&mut self, args: &BuildArgs, is_serve: bool) {
        Self::update_option(&mut self.build.minify, args.minify.as_ref());
        Self::update_option(&mut self.build.css.tailwind.enable, args.tailwind.as_ref());
        self.build.clean = args.clean;

        if is_serve {
            // Serve: disable rss/sitemap by default, enable only if explicitly requested
            self.build.rss.enable = args.rss.unwrap_or(false);
            self.build.sitemap.enable = args.sitemap.unwrap_or(false);
        } else {
            // Build/Deploy: respect config, override only if CLI flag provided
            Self::update_option(&mut self.build.rss.enable, args.rss.as_ref());
            Self::update_option(&mut self.build.sitemap.enable, args.sitemap.as_ref());
        }
    }

    /// Apply serve-specific options.
    fn apply_serve_options(
        &mut self,
        interface: Option<&String>,
        port: Option<u16>,
        watch: Option<bool>,
    ) {
        Self::update_option(&mut self.serve.interface, interface);
        Self::update_option(&mut self.serve.port, port.as_ref());
        Self::update_option(&mut self.serve.watch, watch.as_ref());

        // Set base URL for local development
        self.base.url = Some(format!(
            "http://{}:{}",
            self.serve.interface, self.serve.port
        ));
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
        Self::update_option(&mut self.build.assets, cli.assets.as_ref());
        Self::update_option(&mut self.build.output, cli.output.as_ref());

        // Normalize root to absolute path
        let root = Self::normalize_path(root);
        self.set_root(&root);

        // Normalize config path (already set in main.rs, just canonicalize)
        self.config_path = Self::normalize_path(&self.config_path);

        // Normalize build directories
        self.build.content = Self::normalize_path(&root.join(&self.build.content));
        self.build.assets = Self::normalize_path(&root.join(&self.build.assets));
        self.build.output = Self::normalize_path(&root.join(&self.build.output));
        self.build.templates = Self::normalize_path(&root.join(&self.build.templates));
        self.build.utils = Self::normalize_path(&root.join(&self.build.utils));
        self.build.rss.path = self.build.output.join(&self.build.rss.path);
        self.build.sitemap.path = self.build.output.join(&self.build.sitemap.path);

        // Normalize optional paths
        self.normalize_optional_paths(&root);

        // Normalize misc settings
        self.build.typst.svg.inline_max_size = self.build.typst.svg.inline_max_size.to_uppercase();
    }

    /// Normalize optional paths (tailwind input, deploy token).
    fn normalize_optional_paths(&mut self, root: &Path) {
        if let Some(input) = self.build.css.tailwind.input.take() {
            self.build.css.tailwind.input = Some(Self::normalize_path(&root.join(input)));
        }

        if let Some(token_path) = self.deploy.github.token_path.take() {
            self.deploy.github.token_path = Some(Self::normalize_token_path(&token_path, root));
        }
    }

    /// Normalize token path with tilde expansion.
    fn normalize_token_path(path: &Path, root: &Path) -> PathBuf {
        let expanded = shellexpand::tilde(path.to_str().unwrap_or_default()).into_owned();
        let path = PathBuf::from(expanded);
        let full_path = if path.is_relative() { root.join(&path) } else { path };
        Self::normalize_path(&full_path)
    }

    /// Normalize a path to absolute, using canonicalize if the path exists.
    fn normalize_path(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
            }
        })
    }

    // ========================================================================
    // validation
    // ========================================================================

    /// Validate configuration for the current command.
    pub fn validate(&self) -> Result<()> {
        if !self.config_path.exists() {
            bail!("Config file not found");
        }
        self.validate_base()?;
        self.validate_build()?;
        self.validate_command_specific()?;
        Ok(())
    }

    fn validate_base(&self) -> Result<()> {
        if self.build.rss.enable && self.base.url.is_none() {
            bail!("[base.url] is required for rss generation");
        }

        if let Some(base_url) = &self.base.url
            && !base_url.starts_with("http") {
                bail!(ConfigError::Validation(
                    "[base.url] must start with http:// or https://".into()
                ));
            }
        Ok(())
    }

    fn validate_build(&self) -> Result<()> {
        Self::check_command_installed("[build.typst.command]", &self.build.typst.command)?;
        self.validate_tailwind()?;
        self.validate_inline_max_size()?;
        Ok(())
    }

    fn validate_tailwind(&self) -> Result<()> {
        if !self.build.css.tailwind.enable {
            return Ok(());
        }

        Self::check_command_installed("[build.css.tailwind.command]", &self.build.css.tailwind.command)?;

        match &self.build.css.tailwind.input {
            None => bail!("[build.css.tailwind.enable] = true requires [build.css.tailwind.input] to be set"),
            Some(path) if !path.exists() => {
                bail!(ConfigError::Validation("[build.css.tailwind.input] not found".into()))
            }
            Some(path) if !path.is_file() => {
                bail!(ConfigError::Validation("[build.css.tailwind.input] is not a file".into()))
            }
            _ => Ok(()),
        }
    }

    fn validate_inline_max_size(&self) -> Result<()> {
        const VALID_SUFFIXES: [&str; 3] = ["B", "KB", "MB"];
        if !VALID_SUFFIXES.iter().any(|s| self.build.typst.svg.inline_max_size.ends_with(s)) {
            bail!(ConfigError::Validation(
                "[build.typst.svg.inline_max_size] must end with B, KB, or MB".into()
            ));
        }
        Ok(())
    }

    fn validate_command_specific(&self) -> Result<()> {
        match &self.get_cli().command {
            Commands::Init { .. } if self.get_root().exists() => {
                bail!("Path already exists");
            }
            Commands::Deploy { .. } => self.validate_deploy()?,
            _ => {}
        }
        Ok(())
    }

    fn validate_deploy(&self) -> Result<()> {
        if let Some(path) = &self.deploy.github.token_path {
            if !path.exists() {
                bail!(ConfigError::Validation("[deploy.github.token_path] not found".into()));
            }
            if !path.is_file() {
                bail!(ConfigError::Validation("[deploy.github.token_path] is not a file".into()));
            }
        }
        Ok(())
    }

    /// Check if a command is installed and available.
    fn check_command_installed(field: &str, command: &[String]) -> Result<()> {
        if command.is_empty() {
            bail!(ConfigError::Validation(format!("{field} must have at least one element")));
        }

        let cmd = &command[0];
        which::which(cmd).with_context(|| format!("`{cmd}` not found. Please install it first."))?;
        Ok(())
    }
}

// ============================================================================
// tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size_string() {
        // KB suffix
        assert_eq!(parse_size_string("20KB"), 20 * 1024);
        assert_eq!(parse_size_string("20kb"), 20 * 1024); // case insensitive

        // MB suffix
        assert_eq!(parse_size_string("5MB"), 5 * 1024 * 1024);
        assert_eq!(parse_size_string("1mb"), 1024 * 1024);

        // B suffix
        assert_eq!(parse_size_string("100B"), 100);
        assert_eq!(parse_size_string("256b"), 256);

        // No suffix (defaults to bytes)
        assert_eq!(parse_size_string("100"), 100);

        // Edge cases
        assert_eq!(parse_size_string("0KB"), 0);
        assert_eq!(parse_size_string("invalid"), 0);
    }

    #[test]
    fn test_get_inline_max_size_kb() {
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst.svg]
            inline_max_size = "20KB"
        "#,
        )
        .unwrap();

        assert_eq!(config.get_inline_max_size(), 20 * 1024);
    }

    #[test]
    fn test_get_inline_max_size_mb() {
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst.svg]
            inline_max_size = "5MB"
        "#,
        )
        .unwrap();

        assert_eq!(config.get_inline_max_size(), 5 * 1024 * 1024);
    }

    #[test]
    fn test_get_inline_max_size_bytes() {
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst.svg]
            inline_max_size = "100B"
        "#,
        )
        .unwrap();

        assert_eq!(config.get_inline_max_size(), 100);
    }

    #[test]
    fn test_get_scale_default_dpi() {
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
        "#,
        )
        .unwrap();

        // Default DPI is 96, so scale should be 1.0
        assert_eq!(config.get_scale(), 1.0);
    }

    #[test]
    fn test_get_scale_custom_dpi() {
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst.svg]
            dpi = 192.0
        "#,
        )
        .unwrap();

        // 192 / 96 = 2.0
        assert_eq!(config.get_scale(), 2.0);
    }

    #[test]
    fn test_from_str() {
        let config_str = r#"
            [base]
            title = "My Blog"
            description = "A test blog"
            author = "Test Author"
        "#;
        let result = SiteConfig::from_str(config_str);

        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.base.title, "My Blog");
        assert_eq!(config.base.author, "Test Author");
    }

    #[test]
    fn test_from_str_invalid_toml() {
        let invalid_config = r#"
            [base
            title = "My Blog"
        "#;
        let result = SiteConfig::from_str(invalid_config);

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
    fn test_extra_fields() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [extra]
            custom_field = "custom_value"
            number_field = 42
            nested = { key = "value" }
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(
            config.extra.get("custom_field").and_then(|v| v.as_str()),
            Some("custom_value")
        );
        assert_eq!(
            config
                .extra
                .get("number_field")
                .and_then(|v| v.as_integer()),
            Some(42)
        );
    }

    #[test]
    fn test_extra_fields_nested() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"

            [extra]
            [extra.social]
            twitter = "@user"
            github = "username"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        let social = config.extra.get("social").and_then(|v| v.as_table());
        assert!(social.is_some());
        let social = social.unwrap();
        assert_eq!(
            social.get("twitter").and_then(|v| v.as_str()),
            Some("@user")
        );
        assert_eq!(
            social.get("github").and_then(|v| v.as_str()),
            Some("username")
        );
    }

    #[test]
    fn test_extra_fields_array() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"

            [extra]
            tags = ["rust", "typst", "blog"]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        let tags = config.extra.get("tags").and_then(|v| v.as_array());
        assert!(tags.is_some());
        let tags: Vec<&str> = tags.unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(tags, vec!["rust", "typst", "blog"]);
    }

    #[test]
    fn test_extra_fields_bool_and_float() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"

            [extra]
            show_comments = true
            version = 1.5
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(
            config.extra.get("show_comments").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            config.extra.get("version").and_then(|v| v.as_float()),
            Some(1.5)
        );
    }

    #[test]
    fn test_site_config_default() {
        let config = SiteConfig::default();

        assert!(config.cli.is_none());
        assert_eq!(config.config_path, PathBuf::new());
        assert_eq!(config.base.title, "");
        assert!(config.build.minify);
        assert_eq!(config.serve.port, 5277);
        assert_eq!(config.deploy.provider, "github");
    }

    #[test]
    fn test_parse_size_string_with_spaces() {
        assert_eq!(parse_size_string(" 20 KB"), 20 * 1024);
        assert_eq!(parse_size_string("5 MB"), 5 * 1024 * 1024);
    }

    #[test]
    fn test_full_config_all_sections() {
        let config = r#"
            [base]
            title = "My Blog"
            description = "A personal blog"
            author = "Alice"
            email = "alice@example.com"
            url = "https://myblog.com"
            language = "en-US"
            copyright = "2025 Alice"

            [build]
            content = "posts"
            output = "dist"
            minify = true

            [build.rss]
            enable = true
            path = "rss.xml"

            [build.slug]
            path = "full"
            fragment = "safe"

            [build.typst]
            command = ["typst"]
            [build.typst.svg]
            extract_type = "embedded"
            inline_max_size = "50KB"
            dpi = 144.0

            [build.css.tailwind]
            enable = false

            [serve]
            interface = "127.0.0.1"
            port = 3000
            watch = true

            [deploy]
            provider = "github"
            force = false
            [deploy.github]
            url = "https://github.com/alice/blog"
            branch = "main"

            [extra]
            analytics_id = "UA-12345"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        // Verify all sections loaded correctly
        assert_eq!(config.base.title, "My Blog");
        assert_eq!(config.base.author, "Alice");
        assert_eq!(config.build.content, PathBuf::from("posts"));
        assert!(config.build.rss.enable);
        assert_eq!(config.serve.port, 3000);
        assert_eq!(config.deploy.github.url, "https://github.com/alice/blog");
        assert!(config.extra.contains_key("analytics_id"));
    }

    #[test]
    fn test_unknown_top_level_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"

            [unknown_section]
            field = "value"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }
}
