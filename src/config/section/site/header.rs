//! Custom HTML header configuration.

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::config::ConfigDiagnostics;
use crate::config::PublicUrl;
use crate::config::SiteConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.header")]
pub struct HeaderConfig {
    /// Inject a dummy script to prevent FOUC (Flash of Unstyled Content).
    /// The script blocks rendering briefly, giving CSS time to load.
    pub no_fouc: bool,
    /// Favicon public URL.
    pub icon: Option<PublicUrl>,
    /// CSS stylesheet public URLs.
    pub styles: Vec<PublicUrl>,
    /// Script entries.
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
    /// Validate header resources are public URLs.
    pub fn validate(&self, config: &SiteConfig, diag: &mut ConfigDiagnostics) {
        if let Some(icon) = &self.icon {
            validate_asset_url(icon, Self::FIELDS.icon, config, diag);
        }

        for style in &self.styles {
            validate_asset_url(style, Self::FIELDS.styles, config, diag);
        }

        for script in &self.scripts {
            validate_asset_url(script.url(), Self::FIELDS.scripts, config, diag);
        }
    }
}

fn validate_asset_url(
    url: &PublicUrl,
    field: crate::config::FieldPath,
    config: &SiteConfig,
    diag: &mut ConfigDiagnostics,
) {
    if !url.as_str().trim().starts_with('/') {
        let hint = source_path_url_hint(url.as_str(), config)
            .or_else(|| crate::asset::asset_source_hint(Path::new(url.as_str()), config));
        diag.error_with_hint(
            field,
            format!("header resource '{}' must be a public URL", url.as_str()),
            hint.unwrap_or_else(|| {
                "declare the asset in build.assets, then reference its public URL".into()
            }),
        );
        return;
    }

    let before = diag.len();
    url.validate(field, diag);
    if diag.len() != before {
        return;
    }

    if let Some(hint) = source_path_url_hint(url.as_str(), config) {
        diag.hint(field, hint);
    }
}

fn source_path_url_hint(value: &str, config: &SiteConfig) -> Option<String> {
    let route = crate::asset::route_from_config_source(Path::new(value), config).ok()?;
    Some(format!(
        "did you mean `{}`? Header resources use public URLs; source files are declared in build.assets",
        route.url
    ))
}

// ============================================================================
// Script Entry
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptEntry {
    /// Simple path string.
    Simple(PublicUrl),
    /// URL with `defer`/`async` attributes.
    WithOptions {
        url: PublicUrl,
        #[serde(default)]
        defer: bool,
        #[serde(default)]
        r#async: bool,
    },
}

impl ScriptEntry {
    /// Get the URL for this script entry.
    pub fn url(&self) -> &PublicUrl {
        match self {
            Self::Simple(url) | Self::WithOptions { url, .. } => url,
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
    { url = "/a.js", defer = true },
    "/b.js",
    { url = "/c.js", async = true }
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
    fn header_url_validation_hints_source_path() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("assets")).unwrap();
        fs::create_dir_all(dir.path().join("images")).unwrap();
        fs::write(dir.path().join("images/favicon.ico"), "icon").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.nested = vec![NestedEntry::new(dir.path().join("assets"), "/assets")];
        config.site.header.icon = Some("images/favicon.ico".into());

        let mut diag = ConfigDiagnostics::new();
        config.site.header.validate(&config, &mut diag);

        assert_eq!(diag.len(), 1);
        let error = &diag.errors()[0];
        assert_eq!(error.field, HeaderConfig::FIELDS.icon);
        assert!(error.message.contains("public URL"));
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("build.assets.nested"))
        );
    }

    #[test]
    fn header_styles_reference_public_asset_urls() {
        let dir = TempDir::new().unwrap();
        let styles = dir.path().join("assets/styles");
        fs::create_dir_all(&styles).unwrap();
        fs::write(styles.join("tailwind.css"), "body{}").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.nested = vec![NestedEntry::new(styles, "/styles")];
        config.site.header.styles = vec!["/styles/tailwind.css".into()];

        let mut diag = ConfigDiagnostics::new();
        config.site.header.validate(&config, &mut diag);

        assert!(diag.is_empty(), "{:?}", diag.errors());
    }

    #[test]
    fn header_styles_reject_source_paths_with_url_hint() {
        let dir = TempDir::new().unwrap();
        let styles = dir.path().join("assets/styles");
        fs::create_dir_all(&styles).unwrap();
        fs::write(styles.join("tailwind.css"), "body{}").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.nested = vec![NestedEntry::new(styles, "/styles")];
        config.site.header.styles = vec!["assets/styles/tailwind.css".into()];

        let mut diag = ConfigDiagnostics::new();
        config.site.header.validate(&config, &mut diag);

        assert_eq!(diag.len(), 1);
        let error = &diag.errors()[0];
        assert_eq!(error.field, HeaderConfig::FIELDS.styles);
        assert!(error.message.contains("public URL"));
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("/styles/tailwind.css"))
        );
    }
}
