//! Custom HTML header configuration.

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::ConfigDiagnostics;
use crate::config::SiteConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.header")]
pub struct HeaderConfig {
    /// Inject a dummy script to prevent FOUC (Flash of Unstyled Content).
    /// The script blocks rendering briefly, giving CSS time to load.
    pub no_fouc: bool,
    /// Favicon path (relative to site root).
    pub icon: Option<PathBuf>,
    /// CSS stylesheet paths (relative to site root).
    pub styles: Vec<PathBuf>,
    /// Script entries (relative to site root).
    pub scripts: Vec<ScriptEntry>,
    /// Raw HTML elements to insert into head.
    pub elements: Vec<String>,
}

impl Default for HeaderConfig {
    fn default() -> Self {
        Self {
            no_fouc: true,
            icon: None,
            styles: Vec::new(),
            scripts: Vec::new(),
            elements: Vec::new(),
        }
    }
}

impl HeaderConfig {
    /// Validate all header paths are within configured asset entries.
    pub fn validate(&self, config: &SiteConfig, diag: &mut ConfigDiagnostics) {
        if let Some(icon) = &self.icon {
            validate_asset_source(icon, Self::FIELDS.icon, config, diag);
        }

        for style in &self.styles {
            validate_asset_source(style, Self::FIELDS.styles, config, diag);
        }

        for script in &self.scripts {
            validate_asset_source(script.path(), Self::FIELDS.scripts, config, diag);
        }
    }
}

fn validate_asset_source(
    path: &Path,
    field: crate::config::FieldPath,
    config: &SiteConfig,
    diag: &mut ConfigDiagnostics,
) {
    if crate::asset::route_from_config_source(path, config).is_ok() {
        return;
    }

    let message = format!(
        "path '{}' not in any configured asset entry",
        path.display()
    );
    if let Some(hint) = crate::asset::asset_source_hint(path, config) {
        diag.error_with_hint(field, message, hint);
    } else {
        diag.error(field, message);
    }
}

// ============================================================================
// Script Entry
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptEntry {
    /// Simple path string.
    Simple(PathBuf),
    /// Path with `defer`/`async` attributes.
    WithOptions {
        path: PathBuf,
        #[serde(default)]
        defer: bool,
        #[serde(default)]
        r#async: bool,
    },
}

impl ScriptEntry {
    /// Get the path for this script entry.
    pub fn path(&self) -> &Path {
        match self {
            Self::Simple(path) | Self::WithOptions { path, .. } => path,
        }
    }

    /// Check if defer attribute should be added.
    pub const fn is_defer(&self) -> bool {
        match self {
            Self::Simple(_) => false,
            Self::WithOptions { defer, .. } => *defer,
        }
    }

    /// Check if async attribute should be added.
    pub const fn is_async(&self) -> bool {
        match self {
            Self::Simple(_) => false,
            Self::WithOptions { r#async, .. } => *r#async,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HeaderConfig;
    use crate::config::section::build::assets::NestedEntry;
    use crate::config::{ConfigDiagnostics, SiteConfig, test_parse_config};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scripts_parsing_cases() {
        let config = test_parse_config(
            r#"[site.header]
scripts = [
    { path = "a.js", defer = true },
    "b.js",
    { path = "c.js", async = true }
]"#,
        );
        assert_eq!(config.site.header.scripts.len(), 3);

        // defer script
        assert!(config.site.header.scripts[0].is_defer());
        assert!(!config.site.header.scripts[0].is_async());

        // simple script
        assert!(!config.site.header.scripts[1].is_defer());
        assert!(!config.site.header.scripts[1].is_async());

        // async script
        assert!(!config.site.header.scripts[2].is_defer());
        assert!(config.site.header.scripts[2].is_async());
    }

    #[test]
    fn header_asset_validation_hints_existing_unconfigured_source() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("assets")).unwrap();
        fs::create_dir_all(dir.path().join("images")).unwrap();
        fs::write(dir.path().join("images/favicon.ico"), "icon").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.nested = vec![NestedEntry::Simple(dir.path().join("assets"))];
        config.site.header.icon = Some("images/favicon.ico".into());

        let mut diag = ConfigDiagnostics::new();
        config.site.header.validate(&config, &mut diag);

        assert_eq!(diag.len(), 1);
        let error = &diag.errors()[0];
        assert_eq!(error.field, HeaderConfig::FIELDS.icon);
        assert!(error.message.contains("not in any configured asset entry"));
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("nested = [\"images\"]"))
        );
    }
}
