//! Configuration file generation.
//!
//! Creates tola.toml and ignore files for new sites.

use anyhow::{Context, Result};
use std::{fs, path::Path};

use crate::config::section::{
    AssetsConfig, AssetsValidateConfig, FeedConfig, PagesValidateConfig, ServeConfig,
    build::CssProcessorConfig,
    site::{HeaderConfig, SiteInfoConfig, SitemapConfig},
};
use crate::embed::typst::TOLA_TYP;

/// Default config filename
const CONFIG_FILE: &str = "tola.toml";

/// Files to write ignore patterns to
const IGNORE_FILES: &[&str] = &[".gitignore", ".ignore"];

/// Generate tola.toml content with comments
pub fn generate_config_template() -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "# Tola configuration file (v{})\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str("# https://github.com/tola-rs/tola-ssg\n\n");

    // [site.info] section
    out.push_str(&SiteInfoConfig::template_with_header());
    out.push('\n');

    // [site.feed] section
    out.push_str(&FeedConfig::template_with_header());
    out.push('\n');

    // [site.sitemap] section
    out.push_str(&SitemapConfig::template_with_header());
    out.push('\n');

    // [site.header] section
    out.push_str(&HeaderConfig::template_with_header());
    out.push('\n');

    // [build.assets] section
    out.push_str(&AssetsConfig::template_with_header());
    out.push('\n');

    // [build.hooks.css] section
    out.push_str(&CssProcessorConfig::template_with_header());
    out.push('\n');

    // [serve] section
    out.push_str(&ServeConfig::template_with_header());
    out.push('\n');

    // [validate.pages] section
    out.push_str(&PagesValidateConfig::template_with_header());
    out.push('\n');

    // [validate.assets] section
    out.push_str(&AssetsValidateConfig::template_with_header());

    out
}

/// Write default tola.toml configuration
pub fn write_config(root: &Path) -> Result<()> {
    let content = generate_config_template();

    let path = root.join(CONFIG_FILE);
    fs::write(&path, content)
        .with_context(|| format!("Failed to write config file '{}'", path.display()))?;

    Ok(())
}

/// Write .gitignore and .ignore files with standard patterns
///
/// Patterns include:
/// - Output directory (e.g., `/dist/`)
/// - Tola cache directory (`/.tola/`)
/// - OS-specific files (`.DS_Store`)
pub fn write_ignore_files(root: &Path, output_dir: &Path) -> Result<()> {
    let output_pattern = Path::new("/").join(output_dir);
    let patterns = [
        output_pattern.to_string_lossy().into_owned(),
        "/.tola/".to_string(),
        ".DS_Store".to_string(),
    ];

    let content = patterns.join("\n");

    for filename in IGNORE_FILES {
        let path = root.join(filename);
        // Only create if doesn't exist (don't overwrite user's ignore files)
        if !path.exists() {
            fs::write(&path, &content)
                .with_context(|| format!("Failed to write '{}'", path.display()))?;
        }
    }

    Ok(())
}

/// Write templates/tola.typ with default show rules for HTML export
pub fn write_tola_template(root: &Path) -> Result<()> {
    let path = root.join("templates/tola.typ");
    // Only create if doesn't exist
    if !path.exists() {
        fs::write(&path, TOLA_TYP)
            .with_context(|| format!("Failed to write '{}'", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_write_config() {
        let temp = TempDir::new().unwrap();
        write_config(temp.path()).unwrap();

        let config_path = temp.path().join("tola.toml");
        assert!(config_path.exists());

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("[site.info]"));
        assert!(content.contains("[build.feed]"));
    }

    #[test]
    fn test_write_ignore_files() {
        let temp = TempDir::new().unwrap();
        write_ignore_files(temp.path(), Path::new("dist")).unwrap();

        let gitignore = temp.path().join(".gitignore");
        assert!(gitignore.exists());

        let content = fs::read_to_string(&gitignore).unwrap();
        assert!(content.contains("/dist"));
        assert!(content.contains("/.tola/"));
    }

    #[test]
    fn test_ignore_files_not_overwritten() {
        let temp = TempDir::new().unwrap();
        let gitignore = temp.path().join(".gitignore");
        fs::write(&gitignore, "custom content").unwrap();

        write_ignore_files(temp.path(), Path::new("dist")).unwrap();

        let content = fs::read_to_string(&gitignore).unwrap();
        assert_eq!(content, "custom content");
    }
}
