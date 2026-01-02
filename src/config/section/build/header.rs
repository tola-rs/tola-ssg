//! Custom HTML header configuration.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use macros::Config;

use super::assets::AssetsConfig;
use crate::config::ConfigDiagnostics;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.header")]
pub struct HeaderConfig {
    /// Favicon path (relative to site root).
    pub icon: Option<PathBuf>,
    /// CSS stylesheet paths (relative to site root).
    pub styles: Vec<PathBuf>,
    /// Script entries (relative to site root).
    pub scripts: Vec<ScriptEntry>,
    /// Raw HTML elements to insert into head.
    pub elements: Vec<String>,
}

impl HeaderConfig {
    /// Validate all header paths are within configured asset entries.
    pub fn validate(&self, assets: &AssetsConfig, root: &Path, diag: &mut ConfigDiagnostics) {
        let checker = AssetPathChecker::new(assets, root);

        if let Some(icon) = &self.icon {
            checker.validate(icon, Self::FIELDS.icon, diag);
        }

        for style in &self.styles {
            checker.validate(style, Self::FIELDS.styles, diag);
        }

        for script in &self.scripts {
            checker.validate(script.path(), Self::FIELDS.scripts, diag);
        }
    }
}

// ============================================================================
// Asset Path Checker (Validation Helper)
// ============================================================================

/// Helper to validate paths are within asset configuration.
struct AssetPathChecker<'a> {
    assets: &'a AssetsConfig,
    root: &'a Path,
}

impl<'a> AssetPathChecker<'a> {
    fn new(assets: &'a AssetsConfig, root: &'a Path) -> Self {
        Self { assets, root }
    }

    /// Validate a path is within configured assets, report error if not.
    fn validate(
        &self,
        path: &Path,
        field: crate::config::FieldPath,
        diag: &mut ConfigDiagnostics,
    ) {
        if !self.is_in_assets(path) {
            diag.error(
                field,
                format!("path '{}' not in any configured asset entry", path.display()),
            );
        }
    }

    /// Check if path is within any configured asset entry.
    fn is_in_assets(&self, path: &Path) -> bool {
        let normalized = path.strip_prefix("./").unwrap_or(path);
        let abs_path = crate::utils::path::normalize_path(&self.root.join(normalized));

        // Check flatten (exact match) first, then nested (prefix match)
        self.assets.flatten.iter().any(|e| abs_path == e.source())
            || self.assets.nested.iter().any(|e| abs_path.starts_with(e.source()))
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
    use super::*;
    use crate::config::test_parse_config;

    #[test]
    fn test_defaults() {
        let config = test_parse_config("");
        assert!(config.build.header.icon.is_none());
        assert!(config.build.header.styles.is_empty());
        assert!(config.build.header.scripts.is_empty());
        assert!(config.build.header.elements.is_empty());
    }

    #[test]
    fn test_icon() {
        let config = test_parse_config("[build.header]\nicon = \"images/favicon.avif\"");
        assert_eq!(config.build.header.icon, Some(PathBuf::from("images/favicon.avif")));
    }

    #[test]
    fn test_styles() {
        let config = test_parse_config(
            "[build.header]\nstyles = [\"fonts/font.css\", \"styles/highlight.css\"]",
        );
        assert_eq!(config.build.header.styles.len(), 2);
        assert_eq!(config.build.header.styles[0], PathBuf::from("fonts/font.css"));
        assert_eq!(config.build.header.styles[1], PathBuf::from("styles/highlight.css"));
    }

    #[test]
    fn test_scripts_simple() {
        let config = test_parse_config(
            "[build.header]\nscripts = [\"scripts/a.js\", \"scripts/b.js\"]",
        );
        assert_eq!(config.build.header.scripts.len(), 2);
        assert_eq!(config.build.header.scripts[0].path(), Path::new("scripts/a.js"));
        assert!(!config.build.header.scripts[0].is_defer());
        assert!(!config.build.header.scripts[0].is_async());
    }

    #[test]
    fn test_scripts_with_options() {
        let config = test_parse_config(
            r#"[build.header]
scripts = [
    { path = "a.js", defer = true },
    "b.js",
    { path = "c.js", async = true }
]"#,
        );
        assert_eq!(config.build.header.scripts.len(), 3);

        // defer script
        assert!(config.build.header.scripts[0].is_defer());
        assert!(!config.build.header.scripts[0].is_async());

        // simple script
        assert!(!config.build.header.scripts[1].is_defer());
        assert!(!config.build.header.scripts[1].is_async());

        // async script
        assert!(!config.build.header.scripts[2].is_defer());
        assert!(config.build.header.scripts[2].is_async());
    }

    #[test]
    fn test_elements() {
        let config = test_parse_config(
            r###"[build.header]
elements = ['<meta name="darkreader-lock">', '<meta name="theme-color" content="#fff">']"###,
        );
        assert_eq!(config.build.header.elements.len(), 2);
        assert_eq!(config.build.header.elements[0], "<meta name=\"darkreader-lock\">");
    }

    #[test]
    fn test_script_entry_methods() {
        let simple = ScriptEntry::Simple(PathBuf::from("script.js"));
        assert_eq!(simple.path(), Path::new("script.js"));
        assert!(!simple.is_defer());
        assert!(!simple.is_async());

        let both = ScriptEntry::WithOptions {
            path: PathBuf::from("both.js"),
            defer: true,
            r#async: true,
        };
        assert!(both.is_defer());
        assert!(both.is_async());
    }
}
