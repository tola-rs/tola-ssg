//! `[build]` section configuration.
//!
//! Contains build settings including paths, minification, and sub-configurations.
//!
//! # Example
//!
//! ```toml
//! [build]
//! content = "content"         # Source directory for .typ files (relative to site root)
//! output = "public"           # Output directory for generated HTML (relative to site root)
//! assets = "assets"           # Static assets directory (relative to site root)
//! deps = ["tola", "templates", "utils"] # Dependency dirs (relative to site root)
//! minify = true               # Minify HTML output
//! extra_hints = true          # Show Tola-specific diagnostic hints
//!
//! [build.slug]
//! path = "safe"               # URL path slugification: full | safe | ascii
//! fragment = "full"           # Anchor slugification
//!
//! [build.svg]
//! external = true             # Extract to separate files (false = embed in HTML)
//! converter = "builtin"       # Conversion tool: builtin | magick | ffmpeg | none
//! format = "svg"             # Output format: svg | png | jpg | webp
//! dpi = 144.0                 # Rendering DPI (default: 96.0)
//! ```
//!
//! See submodules for detailed options: [`slug`], [`svg`], [`hooks`].

pub mod assets;
mod atomic;
mod css;
mod diagnostics;
mod hooks;
mod meta;
mod slug;
mod svg;

pub use assets::AssetsConfig;
pub use atomic::AtomicCssConfig;
pub use css::CssConfig;
pub use diagnostics::DiagnosticsConfig;
#[cfg(test)]
pub use hooks::WatchMode;
pub use hooks::{HookConfig, HooksConfig};
pub use meta::MetaConfig;
pub use slug::{SlugCase, SlugConfig, SlugMode};
pub use svg::{SvgConfig, SvgConverter, SvgFormat};

use crate::config::ConfigDiagnostics;
use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build")]
pub struct BuildSectionConfig {
    /// URL path prefix for subdirectory deployment.
    /// Automatically extracted from `[base].url` path component.
    #[serde(skip)]
    #[config(skip)]
    pub path_prefix: PathBuf,

    /// Content source directory (Typst files).
    pub content: PathBuf,

    /// Build output directory.
    pub output: PathBuf,

    /// Static assets configuration.
    pub assets: AssetsConfig,

    /// Dependency directories (Tola library, templates, utilities, etc.).
    pub deps: Vec<PathBuf>,

    /// Virtual data files directory (relative to output).
    pub data: PathBuf,

    /// Minify HTML output.
    pub minify: bool,

    /// Clean output directory before building (CLI only).
    #[serde(skip)]
    #[config(skip)]
    pub clean: bool,

    /// Skip draft pages during build (CLI only).
    #[serde(skip)]
    #[config(skip)]
    pub skip_drafts: bool,

    /// URL slugification settings.
    pub slug: SlugConfig,

    /// SVG processing settings.
    pub svg: SvgConfig,

    /// Build hooks (pre/post commands).
    pub hooks: HooksConfig,

    /// CSS build settings.
    pub css: CssConfig,

    /// Metadata extraction settings.
    pub meta: MetaConfig,

    /// Diagnostics display settings (warnings/errors).
    pub diagnostics: DiagnosticsConfig,

    /// Allow experimental features without warnings.
    #[serde(default)]
    pub allow_experimental: bool,

    /// Show Tola-specific diagnostic hints.
    #[serde(default = "default_extra_hints")]
    pub extra_hints: bool,
}

impl Default for BuildSectionConfig {
    fn default() -> Self {
        Self {
            path_prefix: PathBuf::new(),
            content: "content".into(),
            output: "public".into(),
            assets: AssetsConfig::default(),
            deps: vec!["tola".into(), "templates".into(), "utils".into()],
            data: "_data".into(),
            minify: true,
            clean: false,
            skip_drafts: false,
            slug: SlugConfig::default(),
            svg: SvgConfig::default(),
            hooks: HooksConfig::default(),
            css: CssConfig::default(),
            meta: MetaConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            allow_experimental: false,
            extra_hints: true,
        }
    }
}

const fn default_extra_hints() -> bool {
    true
}

impl BuildSectionConfig {
    /// Validate build configuration.
    ///
    /// Checks deps paths exist and warns about missing ones.
    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        // Warn about missing deps directories
        for dep in &self.deps {
            if !dep.exists() {
                let rel_path = dep
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| dep.display().to_string());
                diag.hint(
                    Self::FIELDS.deps,
                    format!("directory '{}' not found, skipping", rel_path),
                );
            }
        }
        self.css.atomic.validate(diag);
        self.validate_atomic_css_output_source_overlap(diag);
    }

    /// Filter deps to only existing directories.
    ///
    /// Call after validate() to remove missing paths from watch list.
    pub fn filter_existing_deps(&mut self) {
        self.deps.retain(|p| p.exists());
    }

    fn validate_atomic_css_output_source_overlap(&self, diag: &mut ConfigDiagnostics) {
        if !self.css.atomic.enable {
            return;
        }
        let output = crate::utils::path::normalize_path(&AtomicCssConfig::output_path(
            crate::config::PathResolver::new(&self.output, &self.path_prefix),
        ));

        if path_is_inside(&output, &self.content) {
            diag.error(
                Self::FIELDS.output,
                format!(
                    "Atomic CSS output '{}' is inside content source '{}'",
                    output.display(),
                    self.content.display()
                ),
            );
        }

        for source in &self.deps {
            if path_is_inside(&output, source) {
                diag.error(
                    Self::FIELDS.output,
                    format!(
                        "Atomic CSS output '{}' is inside dependency source '{}'",
                        output.display(),
                        source.display()
                    ),
                );
            }
        }

        for source in self.assets.nested_sources() {
            if path_is_inside(&output, source) {
                diag.error(
                    Self::FIELDS.output,
                    format!(
                        "Atomic CSS output '{}' is inside configured asset source '{}'",
                        output.display(),
                        source.display()
                    ),
                );
            }
        }

        for source in self.assets.flatten_sources() {
            if paths_equal(&output, source) {
                diag.error(
                    Self::FIELDS.output,
                    format!(
                        "Atomic CSS output '{}' conflicts with configured file asset '{}'",
                        output.display(),
                        source.display()
                    ),
                );
            }
        }

        if let Some(entries) = &self.css.atomic.source {
            for entry in entries {
                if path_is_inside(&output, entry) {
                    diag.error(
                        AtomicCssConfig::FIELDS.source,
                        format!(
                            "Atomic CSS output '{}' is inside Atomic CSS source '{}'",
                            output.display(),
                            entry.display()
                        ),
                    );
                }
            }
        }
    }
}

fn path_is_inside(path: &Path, source: &Path) -> bool {
    let path = crate::utils::path::normalize_existing_prefix(path);
    let source = crate::utils::path::normalize_existing_prefix(source);
    path == source || path.starts_with(source)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    crate::utils::path::normalize_existing_prefix(left)
        == crate::utils::path::normalize_existing_prefix(right)
}

#[cfg(test)]
mod tests {
    use super::BuildSectionConfig;
    use crate::config::test_parse_config;
    use std::path::Path;

    #[test]
    fn test_custom_assets() {
        let config = test_parse_config(
            r#"
[build.assets]
nested = ["assets/images", { dir = "vendor/static", as = "/lib" }]
flatten = ["assets/styles/base.css", { file = "assets/CNAME", as = "/CNAME" }]
"#,
        );
        assert_eq!(config.build.assets.nested.len(), 2);
        assert_eq!(
            config.build.assets.nested[0].source(),
            Path::new("assets/images")
        );
        assert_eq!(config.build.assets.nested[0].target().as_str(), "/images");
        assert_eq!(
            config.build.assets.nested[1].source(),
            Path::new("vendor/static")
        );
        assert_eq!(config.build.assets.nested[1].target().as_str(), "/lib");
        assert_eq!(config.build.assets.flatten.len(), 2);
        assert_eq!(
            config.build.assets.flatten[0].source(),
            Path::new("assets/styles/base.css")
        );
        assert_eq!(
            config.build.assets.flatten[0].target().as_str(),
            "/base.css"
        );
        assert_eq!(
            config.build.assets.flatten[1].source(),
            Path::new("assets/CNAME")
        );
        assert_eq!(config.build.assets.flatten[1].target().as_str(), "/CNAME");
        // minify defaults to true, only test assets config here
    }

    #[test]
    fn extra_hints_defaults_to_enabled() {
        let config = test_parse_config("");

        assert!(config.build.extra_hints);
    }

    #[test]
    fn extra_hints_can_be_disabled() {
        let config = test_parse_config(
            r#"
[build]
extra_hints = false
"#,
        );

        assert!(!config.build.extra_hints);
    }

    #[test]
    fn atomic_css_config_parses_explicit_build_settings() {
        let config = test_parse_config(
            r#"
[build.css.atomic]
enable = true
profile = "tailwind-v4"
source = ["content", "components/button.typ"]
config = "atomic.css.toml"
"#,
        );

        assert!(config.build.css.atomic.enable);
        assert_eq!(config.build.css.atomic.profile.as_str(), "tailwind-v4");
        assert_eq!(
            config.build.css.atomic.source.as_ref().unwrap(),
            &vec![
                Path::new("content").to_path_buf(),
                Path::new("components/button.typ").to_path_buf()
            ]
        );
        assert_eq!(
            config.build.css.atomic.config.as_deref(),
            Some(Path::new("atomic.css.toml"))
        );
    }

    #[test]
    fn atomic_css_enabled_defaults_to_auto_source_scan() {
        let config = test_parse_config(
            r#"
[build.css.atomic]
enable = true
profile = "tailwind-v4"
"#,
        );
        let mut diag = crate::config::ConfigDiagnostics::new();

        assert!(config.build.css.atomic.source.is_none());

        config.build.validate(&mut diag);

        assert!(diag.errors().is_empty());
    }

    #[test]
    fn atomic_css_explicit_source_must_not_be_empty() {
        let config = test_parse_config(
            r#"
[build.css.atomic]
enable = true
source = []
"#,
        );
        let mut diag = crate::config::ConfigDiagnostics::new();

        config.build.validate(&mut diag);

        let messages: Vec<_> = diag
            .errors()
            .iter()
            .map(|error| error.message.as_str())
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("source must not be empty"))
        );
    }

    #[test]
    fn atomic_css_output_must_not_be_inside_asset_source() {
        let mut config = BuildSectionConfig::default();
        config.output = Path::new("/site").to_path_buf();
        config.assets.nested = vec![crate::config::section::build::assets::NestedEntry::new(
            "/site", "/site",
        )];
        config.css.atomic.enable = true;
        config.css.atomic.source = Some(vec![Path::new("/site/content").to_path_buf()]);
        let mut diag = crate::config::ConfigDiagnostics::new();

        config.validate(&mut diag);

        let messages: Vec<_> = diag
            .errors()
            .iter()
            .map(|error| error.message.as_str())
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("inside configured asset source"))
        );
    }
}
