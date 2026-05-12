//! SEO configuration (feed, sitemap, OG tags).

use crate::config::{ConfigDiagnostics, FieldPath};
use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Feed output format
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FeedFormat {
    /// RSS 2.0 format (default).
    #[default]
    Rss,
    /// Atom 1.0 format.
    Atom,
}

impl FeedFormat {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::Atom => "atom",
        }
    }
}

/// Optional feed output feature.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FeedFeature {
    /// Include full entry HTML in addition to the summary.
    FullText,
}

impl FeedFeature {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullText => "full-text",
        }
    }

    pub const fn requires_feed_body(self) -> bool {
        match self {
            Self::FullText => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Config, PartialEq, Eq)]
#[serde(default)]
#[config(section = "site.seo.feeds")]
pub struct FeedConfig {
    #[config(default = "feed.xml", inline_doc = "Output path for feed file")]
    pub path: PathBuf,
    #[config(default = "rss", inline_doc = "Feed format: rss | atom")]
    pub format: FeedFormat,
    /// Optional feed features.
    pub features: Vec<FeedFeature>,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            path: "feed.xml".into(),
            format: FeedFormat::Rss,
            features: Vec::new(),
        }
    }
}

impl FeedConfig {
    pub fn has_feature(&self, feature: FeedFeature) -> bool {
        self.features.contains(&feature)
    }

    pub fn needs_feed_body(&self) -> bool {
        self.features
            .iter()
            .any(|feature| feature.requires_feed_body())
    }

    pub fn toml_array_table() -> String {
        format!("[[{}]]", Self::TEMPLATE_SECTION)
    }

    pub fn commented_template() -> String {
        let default = Self::default();
        let mut out = String::new();
        out.push_str("# ");
        out.push_str(&Self::toml_array_table());
        out.push('\n');
        out.push_str(&format!(
            "# {} = {}  # rss | atom\n",
            toml_key(Self::FIELDS.format),
            toml::Value::try_from(default.format)
                .map(|v| v.to_string())
                .unwrap_or_default()
        ));
        out.push_str(&format!(
            "# {} = {}\n",
            toml_key(Self::FIELDS.path),
            toml::Value::try_from(default.path)
                .map(|v| v.to_string())
                .unwrap_or_default()
        ));
        out.push_str(&format!(
            "# {} = {}  # {}\n",
            toml_key(Self::FIELDS.features),
            toml::Value::try_from(default.features)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            FeedFeature::FullText.as_str()
        ));
        out
    }
}

fn toml_key(field: FieldPath) -> &'static str {
    field.as_str().rsplit('.').next().unwrap_or(field.as_str())
}

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.seo.sitemap")]
pub struct SitemapConfig {
    #[config(inline_doc = "Enable sitemap generation")]
    pub enable: bool,
    #[config(inline_doc = "Output path for sitemap file")]
    pub path: PathBuf,
}

impl Default for SitemapConfig {
    fn default() -> Self {
        Self {
            enable: false,
            path: "sitemap.xml".into(),
        }
    }
}

/// SEO configuration containing feed, sitemap, and OG tag settings
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.seo")]
pub struct SeoConfig {
    #[config(inline_doc = "Auto-inject OG meta tags (can be overridden in Typst)")]
    pub auto_og: bool,

    /// Feed outputs (RSS/Atom).
    #[config(skip)]
    pub feeds: Vec<FeedConfig>,

    /// Sitemap generation settings
    #[config(sub)]
    pub sitemap: SitemapConfig,
}

impl SeoConfig {
    pub fn has_feed_outputs(&self) -> bool {
        !self.feeds.is_empty()
    }

    pub fn feed_outputs(&self) -> &[FeedConfig] {
        &self.feeds
    }

    pub fn clear_feed_outputs(&mut self) {
        self.feeds.clear();
    }

    pub fn needs_feed_body(&self) -> bool {
        self.feeds.iter().any(FeedConfig::needs_feed_body)
    }

    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        self.validate_features(diag);
    }

    pub fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        for (idx, feed) in self.feeds.iter().enumerate() {
            validate_output_path(
                &feed.path,
                idx,
                self.feeds.len(),
                FeedConfig::FIELDS.path,
                diag,
            );
        }
        validate_output_path(&self.sitemap.path, 0, 1, SitemapConfig::FIELDS.path, diag);
        validate_feed_path_collisions(self, diag);
    }

    fn validate_features(&self, diag: &mut ConfigDiagnostics) {
        for (feed_idx, feed) in self.feeds.iter().enumerate() {
            let mut seen: Vec<(FeedFeature, usize)> = Vec::new();
            for (feature_idx, feature) in feed.features.iter().copied().enumerate() {
                if let Some((_, prev_idx)) = seen.iter().find(|(seen, _)| *seen == feature) {
                    diag.error(
                        FeedConfig::FIELDS.features,
                        format!(
                            "[{feed_idx}] feature '{}' duplicates feature [{prev_idx}]",
                            feature.as_str()
                        ),
                    );
                } else {
                    seen.push((feature, feature_idx));
                }
            }
        }
    }
}

fn validate_feed_path_collisions(seo: &SeoConfig, diag: &mut ConfigDiagnostics) {
    let mut seen: Vec<(&Path, usize)> = Vec::new();

    for (idx, feed) in seo.feeds.iter().enumerate() {
        if let Some((_, prev_idx)) = seen.iter().find(|(path, _)| *path == feed.path.as_path()) {
            diag.error(
                FeedConfig::FIELDS.path,
                format!(
                    "[{idx}] path '{}' duplicates feed output [{prev_idx}]",
                    feed.path.display()
                ),
            );
        } else {
            seen.push((feed.path.as_path(), idx));
        }

        if seo.sitemap.enable && feed.path.as_path() == seo.sitemap.path.as_path() {
            diag.error(
                FeedConfig::FIELDS.path,
                format!(
                    "[{idx}] path '{}' conflicts with enabled sitemap output",
                    feed.path.display()
                ),
            );
        }
    }
}

fn validate_output_path(
    path: &Path,
    idx: usize,
    total: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    for comp in path.components() {
        let msg = match comp {
            Component::ParentDir => Some("parent directory '..' not allowed"),
            Component::Prefix(_) | Component::RootDir => Some("absolute paths not allowed"),
            _ => None,
        };
        if let Some(reason) = msg {
            let prefix = if total > 1 {
                format!("[{idx}] ")
            } else {
                String::new()
            };
            diag.error(
                field,
                format!("{prefix}path '{}': {reason}", path.display()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FeedConfig, SitemapConfig};
    use crate::config::{FeedFeature, FeedFormat, test_parse_config};
    use std::path::PathBuf;

    fn feed_entry(format: &str, path: &str) -> String {
        format!(
            r#"
{}
format = "{format}"
path = "{path}"
"#,
            FeedConfig::toml_array_table()
        )
    }

    fn sitemap_entry(path: &str) -> String {
        format!(
            r#"
[{}]
enable = true
path = "{path}"
"#,
            SitemapConfig::TEMPLATE_SECTION
        )
    }

    #[test]
    fn parses_multiple_feed_outputs() {
        let config = test_parse_config(&format!(
            "{}{}",
            feed_entry("rss", "feed.xml"),
            feed_entry("atom", "atom.xml")
        ));

        assert_eq!(config.site.seo.feeds.len(), 2);
        assert_eq!(config.site.seo.feeds[0].format, FeedFormat::Rss);
        assert_eq!(config.site.seo.feeds[0].path, PathBuf::from("feed.xml"));
        assert_eq!(config.site.seo.feeds[1].format, FeedFormat::Atom);
        assert_eq!(config.site.seo.feeds[1].path, PathBuf::from("atom.xml"));
    }

    #[test]
    fn parses_feed_features() {
        let config = test_parse_config(&format!(
            r#"
{}
format = "rss"
path = "feed.xml"
features = ["full-text"]
"#,
            FeedConfig::toml_array_table()
        ));

        assert_eq!(
            config.site.seo.feeds[0].features,
            vec![FeedFeature::FullText]
        );
    }

    #[test]
    fn rejects_duplicate_feed_features() {
        let config = test_parse_config(&format!(
            r#"
{}
format = "rss"
path = "feed.xml"
features = ["full-text", "full-text"]
"#,
            FeedConfig::toml_array_table()
        ));
        let mut diag = crate::config::ConfigDiagnostics::new();

        config.site.seo.validate(&mut diag);

        assert!(diag.has_errors());
    }

    #[test]
    fn rejects_unsafe_feed_output_paths() {
        let config = test_parse_config(&feed_entry("rss", "../feed.xml"));
        let mut diag = crate::config::ConfigDiagnostics::new();

        config.site.seo.validate_paths(&mut diag);

        assert!(diag.has_errors());
    }

    #[test]
    fn rejects_unsafe_sitemap_output_paths() {
        let config = test_parse_config(&sitemap_entry("../sitemap.xml"));
        let mut diag = crate::config::ConfigDiagnostics::new();

        config.site.seo.validate_paths(&mut diag);

        assert!(diag.has_errors());
    }

    #[test]
    fn rejects_duplicate_feed_output_paths() {
        let config = test_parse_config(&format!(
            "{}{}",
            feed_entry("rss", "feed.xml"),
            feed_entry("atom", "feed.xml")
        ));
        let mut diag = crate::config::ConfigDiagnostics::new();

        config.site.seo.validate_paths(&mut diag);

        assert!(diag.has_errors());
    }

    #[test]
    fn rejects_feed_sitemap_output_path_collision() {
        let config = test_parse_config(&format!(
            "{}{}",
            feed_entry("rss", "sitemap.xml"),
            sitemap_entry("sitemap.xml")
        ));
        let mut diag = crate::config::ConfigDiagnostics::new();

        config.site.seo.validate_paths(&mut diag);

        assert!(diag.has_errors());
    }
}
