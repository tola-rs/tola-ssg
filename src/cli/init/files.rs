//! Initial site files.
//!
//! Writes tola.toml, ignore files, and embedded Typst support files.

use anyhow::{Context, Result};
use std::{fs, path::Path};

use crate::config::section::{
    AssetsConfig, AssetsValidateConfig, PagesValidateConfig, ServeConfig,
    build::AtomicCssConfig,
    site::{FeedConfig, HeaderConfig, SeoConfig, SiteInfoConfig, SitemapConfig},
};
use crate::embed::typst::{TOLA_TEMPLATE, TOLA_UTIL, TolaTypstVars};

use super::Settings;

/// Default config filename
const CONFIG_FILE: &str = "tola.toml";

/// Files to write ignore patterns to
const IGNORE_FILES: &[&str] = &[".gitignore", ".ignore"];

/// Generate tola.toml content with comments
pub fn generate_config_template(settings: &Settings) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "# Tola configuration file (v{})\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str("# https://github.com/tola-rs/tola-ssg\n\n");

    out.push_str(&site_info_template(settings));
    out.push('\n');

    out.push_str(&seo_template(settings));
    out.push('\n');

    if settings.atomic_css {
        out.push_str(&atomic_css_template());
        out.push('\n');
    }

    // Header assets section
    out.push_str(&HeaderConfig::template_with_header());
    out.push('\n');

    // Static assets section
    out.push_str(&AssetsConfig::template_with_header());
    out.push('\n');

    // Development server section
    out.push_str(&ServeConfig::template_with_header());
    out.push('\n');

    // Page validation section
    out.push_str(&PagesValidateConfig::template_with_header());
    out.push('\n');

    // Asset validation section
    out.push_str(&AssetsValidateConfig::template_with_header());

    out
}

/// Write default tola.toml configuration
pub fn write_config(root: &Path, settings: &Settings) -> Result<()> {
    let content = generate_config_template(settings);

    let path = root.join(CONFIG_FILE);
    fs::write(&path, content)
        .with_context(|| format!("Failed to write config file '{}'", path.display()))?;

    Ok(())
}

fn site_info_template(settings: &Settings) -> String {
    let mut out = String::new();
    out.push_str("# Site metadata for feed generation and Typst templates\n");
    out.push_str("# Access in Typst via `#import \"@tola/site:0.0.0\": info`\n");
    out.push_str("# For custom fields, use `[site.info.extra]` and access via `info.extra.xxx`\n");
    out.push_str(&format!("[{}]\n", SiteInfoConfig::TEMPLATE_SECTION));
    out.push_str(&format!(
        "title = {}  # Site title\n",
        toml_string(&settings.title)
    ));
    out.push_str(&format!(
        "author = {}  # Author name\n",
        toml_string(&settings.author)
    ));
    out.push_str(&format!(
        "email = {}  # Author email\n",
        toml_string(&settings.email)
    ));
    out.push_str("description = \"\"  # Site description\n");
    if let Some(url) = &settings.base_url {
        out.push_str(&format!(
            "url = {}  # Site URL, path used as prefix\n",
            toml_string(url)
        ));
    } else {
        out.push_str("# url = \"\"  # Site URL, path used as prefix\n");
    }
    out.push_str(&format!(
        "language = {}  # Language code\n",
        toml_string(&settings.language)
    ));
    out.push_str("copyright = \"\"  # Copyright notice\n");
    out
}

fn seo_template(settings: &Settings) -> String {
    let mut out = String::new();
    out.push_str("# SEO configuration containing feed, sitemap, and OG tag settings\n");
    out.push_str(&format!("[{}]\n", SeoConfig::TEMPLATE_SECTION));
    out.push_str("auto_og = false  # Auto-inject OG meta tags (can be overridden in Typst)\n\n");
    out.push_str("# Sitemap generation settings\n");
    out.push_str(&format!("[{}]\n", SitemapConfig::TEMPLATE_SECTION));
    out.push_str(&format!(
        "enable = {}  # Enable sitemap generation\n",
        settings.sitemap
    ));
    out.push_str("url = \"/sitemap.xml\"  # Public URL for sitemap file\n");

    if settings.feeds.is_empty() {
        out.push('\n');
        out.push_str(&FeedConfig::commented_template());
        return out;
    }

    for feed in &settings.feeds {
        out.push('\n');
        out.push_str(&FeedConfig::toml_array_table());
        out.push('\n');
        out.push_str(&format!("format = {}\n", toml_string(feed.as_str())));
        out.push_str(&format!("url = {}\n", toml_string(feed.url())));
        out.push_str("features = []  # full-text | no-script\n");
    }

    out
}

fn atomic_css_template() -> String {
    let mut out = String::new();
    out.push_str(&format!("[{}]\n", AtomicCssConfig::TEMPLATE_SECTION));
    out.push_str("enable = true  # Enable native Atomic CSS generation\n");
    out.push_str("profile = \"tailwind-v4\"  # Compatibility profile\n");
    out
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
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
        let content = TOLA_TEMPLATE.render(&TolaTypstVars::default());
        fs::write(&path, content)
            .with_context(|| format!("Failed to write '{}'", path.display()))?;
    }
    Ok(())
}

/// Write utils/tola.typ with utility functions
pub fn write_tola_util(root: &Path) -> Result<()> {
    let path = root.join("utils/tola.typ");
    // Only create if doesn't exist
    if !path.exists() {
        let content = TOLA_UTIL.render(&TolaTypstVars::default());
        fs::write(&path, content)
            .with_context(|| format!("Failed to write '{}'", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init::FeedFormat;
    use tempfile::TempDir;

    #[test]
    fn test_write_config() {
        let temp = TempDir::new().unwrap();
        write_config(temp.path(), &Settings::recommended()).unwrap();

        let config_path = temp.path().join("tola.toml");
        assert!(config_path.exists());

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains(&format!("[{}]", SiteInfoConfig::TEMPLATE_SECTION)));
        assert!(content.contains(&FeedConfig::commented_template()));
    }

    #[test]
    fn config_template_uses_settings() {
        let content = generate_config_template(&Settings {
            title: "My Blog".into(),
            base_url: Some("https://example.com/blog".into()),
            language: "zh-CN".into(),
            author: "Kaway".into(),
            email: "kaway@example.com".into(),
            atomic_css: true,
            feeds: vec![FeedFormat::Rss, FeedFormat::Json],
            sitemap: true,
        });

        assert!(content.contains("title = \"My Blog\""));
        assert!(content.contains("url = \"https://example.com/blog\""));
        assert!(content.contains("language = \"zh-CN\""));
        assert!(content.contains("[build.css.atomic]"));
        assert!(content.contains("profile = \"tailwind-v4\""));
        assert!(content.contains("format = \"rss\""));
        assert!(content.contains("url = \"/feed.xml\""));
        assert!(content.contains("format = \"json\""));
        assert!(content.contains("url = \"/feed.json\""));
        assert!(content.contains("enable = true  # Enable sitemap generation"));
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
