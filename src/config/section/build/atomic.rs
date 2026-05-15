//! Native atomic CSS build configuration.

use crate::config::{ConfigDiagnostics, PathResolver};
use crate::core::UrlPath;
use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Native atomic CSS build settings.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.atomic_css")]
pub struct AtomicCssConfig {
    /// Enable Tola Atomic CSS generation.
    #[config(inline_doc = "Enable native Atomic CSS generation")]
    pub enable: bool,
    /// Atomic CSS compatibility profile.
    #[config(default = "tailwind-v4", inline_doc = "Compatibility profile")]
    pub profile: String,
    /// Source roots scanned for atomic class candidates.
    #[serde(default)]
    #[config(inline_doc = "Source paths scanned for atomic classes")]
    pub sources: Vec<PathBuf>,
    /// Optional Atomic CSS semantic config file relative to site root.
    #[config(inline_doc = "e.g. \"atomic.css.toml\"")]
    pub config: Option<PathBuf>,
}

impl Default for AtomicCssConfig {
    fn default() -> Self {
        Self {
            enable: false,
            profile: "tailwind-v4".into(),
            sources: Vec::new(),
            config: None,
        }
    }
}

impl AtomicCssConfig {
    pub const OUTPUT_FILE: &'static str = "atomic.css";

    pub fn output_logical_path() -> PathBuf {
        PathBuf::from(crate::asset::SYSTEM_ASSET_DIR).join(Self::OUTPUT_FILE)
    }

    pub fn output_route() -> UrlPath {
        UrlPath::from_asset(&format!(
            "{}/{}",
            crate::asset::SYSTEM_ASSET_DIR,
            Self::OUTPUT_FILE
        ))
    }

    pub fn output_path(paths: PathResolver<'_>) -> PathBuf {
        paths.output_dir().join(Self::output_logical_path())
    }

    pub fn output_href(paths: PathResolver<'_>) -> String {
        paths.url_for_site_path(Self::output_logical_path())
    }

    /// Validate semantic requirements after path normalization.
    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        if !self.enable {
            return;
        }

        if self.sources.is_empty() {
            diag.error(
                Self::FIELDS.sources,
                "sources is required when atomic CSS is enabled",
            );
        }
        if self.profile != "tailwind-v4" {
            diag.error(
                Self::FIELDS.profile,
                format!("unknown atomic CSS profile `{}`", self.profile),
            );
        }
    }

    /// Validate path safety before site-root normalization.
    pub fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        if let Some(config) = &self.config {
            validate_relative_path(config, Self::FIELDS.config, diag);
        }
        for source in &self.sources {
            validate_relative_path(source, Self::FIELDS.sources, diag);
        }
    }

    /// Normalize site-root relative source/config paths.
    pub fn normalize(&mut self, root: &Path) {
        self.sources = self
            .sources
            .iter()
            .map(|path| crate::utils::path::normalize_path(&root.join(path)))
            .collect();
        if let Some(config) = self.config.take() {
            self.config = Some(crate::utils::path::normalize_path(&root.join(config)));
        }
    }
}

fn validate_relative_path(
    path: &Path,
    field: crate::config::FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    for comp in path.components() {
        let message = match comp {
            Component::ParentDir => Some("parent directory '..' not allowed"),
            Component::Prefix(_) | Component::RootDir => Some("absolute paths not allowed"),
            _ => None,
        };
        if let Some(reason) = message {
            diag.error(field, format!("path '{}': {reason}", path.display()));
        }
    }
}
