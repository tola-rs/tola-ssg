//! SEO configuration (feed, sitemap, OG tags).

use crate::config::{ConfigDiagnostics, FieldPath, GeneratedOutputPath};
use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Feed output format
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FeedFormat {
    /// RSS 2.0 format (default).
    #[default]
    Rss,
    /// Atom 1.0 format.
    Atom,
    /// JSON Feed 1.1 format.
    Json,
}

impl FeedFormat {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::Atom => "atom",
            Self::Json => "json",
        }
    }

    pub const fn mime_type(&self) -> &'static str {
        match self {
            Self::Rss => "application/rss+xml",
            Self::Atom => "application/atom+xml",
            Self::Json => "application/feed+json",
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            Self::Rss => "RSS",
            Self::Atom => "Atom",
            Self::Json => "JSON Feed",
        }
    }
}

/// Optional feed output feature.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FeedFeature {
    /// Include full entry HTML in addition to the summary.
    FullText,
    /// Remove scripts and event handler attributes from feed HTML.
    NoScript,
}

impl FeedFeature {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullText => "full-text",
            Self::NoScript => "no-script",
        }
    }

    pub const fn requires_feed_body(self) -> bool {
        match self {
            Self::FullText => true,
            Self::NoScript => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Config, PartialEq, Eq)]
#[serde(default)]
#[config(section = "site.seo.feeds")]
pub struct FeedConfig {
    #[config(default = "feed.xml", inline_doc = "Output path for feed file")]
    pub output: GeneratedOutputPath,
    #[config(default = "rss", inline_doc = "Feed format: rss | atom | json")]
    pub format: FeedFormat,
    /// Optional feed features.
    pub features: Vec<FeedFeature>,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            output: "feed.xml".into(),
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
            "# {} = {}  # rss | atom | json\n",
            toml_key(Self::FIELDS.format),
            toml::Value::try_from(default.format)
                .map(|v| v.to_string())
                .unwrap_or_default()
        ));
        out.push_str(&format!(
            "# {} = {}\n",
            toml_key(Self::FIELDS.output),
            toml::Value::String(default.output.logical_path())
        ));
        out.push_str(&format!(
            "# {} = {}  # {}\n",
            toml_key(Self::FIELDS.features),
            toml::Value::try_from(default.features)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            format!(
                "{} | {}",
                FeedFeature::FullText.as_str(),
                FeedFeature::NoScript.as_str()
            )
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
    pub output: GeneratedOutputPath,
}

impl Default for SitemapConfig {
    fn default() -> Self {
        Self {
            enable: false,
            output: "sitemap.xml".into(),
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

    /// Feed outputs (RSS/Atom/JSON Feed).
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
            feed.output
                .validate_indexed(FeedConfig::FIELDS.output, idx, self.feeds.len(), diag);
        }
        self.sitemap
            .output
            .validate(SitemapConfig::FIELDS.output, diag);
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
        if let Some((_, prev_idx)) = seen.iter().find(|(path, _)| *path == feed.output.as_path()) {
            diag.error(
                FeedConfig::FIELDS.output,
                format!(
                    "[{idx}] output '{}' duplicates feed output [{prev_idx}]",
                    feed.output
                ),
            );
        } else {
            seen.push((feed.output.as_path(), idx));
        }

        if seo.sitemap.enable && feed.output.as_path() == seo.sitemap.output.as_path() {
            diag.error(
                FeedConfig::FIELDS.output,
                format!(
                    "[{idx}] output '{}' conflicts with enabled sitemap output",
                    feed.output
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FeedConfig, SitemapConfig};
    use crate::config::{FeedFeature, FeedFormat, test_parse_config};
    use std::path::Path;

    fn feed_entry(format: &str, output: &str) -> String {
        format!(
            r#"
{}
format = "{format}"
output = "{output}"
"#,
            FeedConfig::toml_array_table()
        )
    }

    fn legacy_feed_path_entry(format: &str, path: &str) -> String {
        format!(
            r#"
{}
format = "{format}"
path = "{path}"
"#,
            FeedConfig::toml_array_table()
        )
    }

    fn sitemap_entry(output: &str) -> String {
        format!(
            r#"
[{}]
enable = true
output = "{output}"
"#,
            SitemapConfig::TEMPLATE_SECTION
        )
    }

    #[test]
    fn parses_generated_output_fields() {
        let config = test_parse_config(&format!(
            "{}{}",
            feed_entry("rss", "feed.xml"),
            sitemap_entry("sitemap.xml")
        ));

        assert_eq!(
            config.site.seo.feeds[0].output.as_path(),
            Path::new("feed.xml")
        );
        assert_eq!(
            config.site.seo.sitemap.output.as_path(),
            Path::new("sitemap.xml")
        );
    }

    #[test]
    fn rejects_legacy_feed_path_field() {
        let content = crate::config::test_config_source(&legacy_feed_path_entry("rss", "feed.xml"));
        let (_, ignored) = crate::config::SiteConfig::parse_with_ignored(&content).unwrap();

        assert!(ignored.iter().any(|field| field.contains("path")));
    }

    #[test]
    fn parses_multiple_feed_outputs() {
        let config = test_parse_config(&format!(
            "{}{}{}",
            feed_entry("rss", "feed.xml"),
            feed_entry("atom", "atom.xml"),
            feed_entry("json", "feed.json")
        ));

        assert_eq!(config.site.seo.feeds.len(), 3);
        assert_eq!(config.site.seo.feeds[0].format, FeedFormat::Rss);
        assert_eq!(
            config.site.seo.feeds[0].output.as_path(),
            Path::new("feed.xml")
        );
        assert_eq!(config.site.seo.feeds[1].format, FeedFormat::Atom);
        assert_eq!(
            config.site.seo.feeds[1].output.as_path(),
            Path::new("atom.xml")
        );
        assert_eq!(config.site.seo.feeds[2].format, FeedFormat::Json);
        assert_eq!(
            config.site.seo.feeds[2].output.as_path(),
            Path::new("feed.json")
        );
    }

    #[test]
    fn parses_feed_features() {
        let config = test_parse_config(&format!(
            r#"
{}
format = "rss"
output = "feed.xml"
features = ["full-text", "no-script"]
"#,
            FeedConfig::toml_array_table()
        ));

        assert_eq!(
            config.site.seo.feeds[0].features,
            vec![FeedFeature::FullText, FeedFeature::NoScript]
        );
    }

    #[test]
    fn rejects_duplicate_feed_features() {
        let config = test_parse_config(&format!(
            r#"
{}
format = "rss"
output = "feed.xml"
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
