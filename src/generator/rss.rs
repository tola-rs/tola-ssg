//! rss feed generation.
//!
//! Parses post metadata and generates rss/atom feeds.

use crate::{
    config::SiteConfig,
    exec, log,
    utils::{
        date::DateTimeUtc,
        exec::SILENT_FILTER,
        meta::Pages,
        typst::TypstElement,
    },
};
use anyhow::{Context, Ok, Result, anyhow};
use rayon::prelude::*;
use regex::Regex;
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder, validation::Validate};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::LazyLock};

// ============================================================================
// Constants
// ============================================================================

/// Tag name for querying typst metadata
const META_TAG_NAME: &str = "<tola-meta>";



// ============================================================================
// rss feed types
// ============================================================================

/// rss feed builder
pub struct RssFeed {
    title: String,
    description: String,
    base_url: String,
    language: String,
    posts: Vec<PostMeta>,
}

/// Metadata extracted from a post
#[derive(Debug, Default, Serialize, Deserialize)]
struct PostMeta {
    title: Option<String>,
    summary: Option<String>,
    date: Option<String>,
    #[allow(dead_code)]
    update: Option<String>,
    #[serde(default)]
    link: Option<String>,
    author: Option<String>,
}

impl PostMeta {
    /// Convert to rss item, returns None if required fields are missing
    fn into_rss_item(self) -> Option<rss::Item> {
        let title = self.title?;
        let link = self.link.clone()?;
        let pub_date = DateTimeUtc::parse(self.date.as_deref()?).map(|dt| dt.to_rfc2822())?;

        Some(
            ItemBuilder::default()
                .title(title)
                .link(self.link)
                .guid(GuidBuilder::default().permalink(true).value(link).build())
                .description(self.summary)
                .pub_date(pub_date)
                .author(self.author)
                .build(),
        )
    }
}



// ============================================================================
// Public API
// ============================================================================

/// Build rss feed if enabled in config.
///
/// Uses pre-collected page metadata for URLs, but still queries typst
/// for title/summary/date since those require parsing the source files.
pub fn build_rss(config: &SiteConfig, pages: &Pages) -> Result<()> {
    if config.build.rss.enable {
        RssFeed::build(config, pages)?.write(config)?;
    }
    Ok(())
}

// ============================================================================
// RssFeed Implementation
// ============================================================================

impl RssFeed {
    /// Build rss feed using pre-collected page metadata.
    ///
    /// Uses `Pages` for URL information, but queries typst for
    /// title/summary/date metadata in parallel.
    pub fn build(config: &SiteConfig, pages: &Pages) -> Result<Self> {
        // log!("rss"; "generating from {} pages", pages.len());

        // Parallel query for better performance
        let posts: Vec<PostMeta> = pages
            .iter()
            .collect::<Vec<_>>()
            .par_iter()
            .map(|page| query_post_meta(&page.paths.source, &page.paths.full_url, config))
            .collect::<Result<_>>()?;

        Ok(Self {
            title: config.base.title.clone(),
            description: config.base.description.clone(),
            base_url: config.base.url.clone().unwrap_or_default(),
            language: config.base.language.clone(),
            posts,
        })
    }

    /// Generate rss xml string
    fn into_xml(self) -> Result<String> {
        let items: Vec<_> = self
            .posts
            .into_iter()
            .filter_map(PostMeta::into_rss_item)
            .collect();

        let channel = ChannelBuilder::default()
            .title(self.title)
            .link(self.base_url)
            .description(self.description)
            .language(self.language)
            .generator("tola-ssg".to_string())
            .items(items)
            .build();

        channel
            .validate()
            .map_err(|e| anyhow!("rss validation failed: {e}"))?;
        Ok(channel.to_string())
    }

    /// Write rss feed to file
    pub fn write(self, config: &SiteConfig) -> Result<()> {
        let xml = self.into_xml()?;
        let rss_path = &config.build.rss.path;

        if let Some(parent) = rss_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(rss_path, &xml)?;

        log!("rss"; "{}", rss_path.file_name().unwrap_or_default().to_string_lossy());
        Ok(())
    }
}

// ============================================================================
// Metadata Extraction
// ============================================================================

/// Query metadata from a Typst post file.
/// Uses SILENT_FILTER to suppress warnings (already shown during build).
fn query_post_meta(post_path: &Path, guid: &str, config: &SiteConfig) -> Result<PostMeta> {
    let root = config.get_root();

    let output = exec!(
        filter=&SILENT_FILTER;
        &config.build.typst.command;
        "query", "--features", "html", "--format", "json",
        "--font-path", root, "--root", root,
        post_path,
        META_TAG_NAME, "--field", "value", "--one"
    )
    .with_context(|| {
        format!(
            "Failed to query metadata for post: {}\nEnsure tag name \"{}\" is correct",
            post_path.display(),
            META_TAG_NAME
        )
    })?;

    let json_str = std::str::from_utf8(&output.stdout)?;
    parse_post_meta(guid.to_string(), json_str, config)
}

/// Parse post metadata from JSON string
fn parse_post_meta(guid: String, json_str: &str, config: &SiteConfig) -> Result<PostMeta> {
    let json: serde_json::Value = serde_json::from_str(json_str)
        .with_context(|| format!("Failed to parse post metadata JSON:\n{json_str}"))?;

    let get_string = |key: &str| json.get(key).and_then(|v| v.as_str()).map(String::from);

    // Parse summary from Typst element
    let base_url = config.base.url.as_deref().unwrap_or_default();
    let summary = get_string("summary")
        .and_then(|s| parse_typst_element(&s).ok())
        .map(|elem| elem.to_html(base_url));

    // Process author field
    let author = get_string("author");
    let author = normalize_rss_author(author.as_ref(), config);

    Ok(PostMeta {
        title: get_string("title"),
        summary,
        date: get_string("date"),
        update: get_string("update"),
        link: Some(guid),
        author,
    })
}

/// Parse Typst element from JSON string
fn parse_typst_element(content: &str) -> Result<TypstElement> {
    serde_json::from_str(content).map_err(Into::into)
}

/// Normalize author field to rss format: "email@example.com (Name)"
///
/// Priority:
/// 1. Post meta author if already in valid format
/// 2. Site config author if in valid format
/// 3. Combine site config email and author
fn normalize_rss_author(author: Option<&String>, config: &SiteConfig) -> Option<String> {
    static RE_VALID_AUTHOR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}[ \t]*\([^)]+\)$").unwrap()
    });

    let author = author?;

    // Check if post author is already valid
    if RE_VALID_AUTHOR.is_match(author) {
        return Some(author.clone());
    }

    // Try site config author
    let site_author = &config.base.author;
    if RE_VALID_AUTHOR.is_match(site_author) {
        return Some(site_author.clone());
    }

    // Combine email and author name
    Some(format!("{} ({})", config.base.email, site_author))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a config for testing
    fn make_config(author: &str, email: &str) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.base.author = author.to_string();
        config.base.email = email.to_string();
        config.base.url = Some("https://example.com".to_string());
        config
    }

    #[test]
    fn test_normalize_rss_author() {
        let config = make_config("Site Author", "site@example.com");

        // Case 1: Post author is already valid
        let post_author = "post@example.com (Post Author)".to_string();
        assert_eq!(
            normalize_rss_author(Some(&post_author), &config),
            Some(post_author)
        );

        // Case 2: Post author is invalid (just name), fallback to site config (combined)
        let post_author_invalid = "Post Author".to_string();
        assert_eq!(
            normalize_rss_author(Some(&post_author_invalid), &config),
            Some("site@example.com (Site Author)".to_string())
        );

        // Case 3: Post author None, returns None (current behavior)
        assert_eq!(normalize_rss_author(None, &config), None);

        // Case 4: Site author is valid email format
        let config_valid = make_config("site@example.com (Site Author)", "");
        assert_eq!(
            normalize_rss_author(Some(&post_author_invalid), &config_valid),
            Some("site@example.com (Site Author)".to_string())
        );
    }

    #[test]
    fn test_post_meta_into_rss_item() {
        let meta = PostMeta {
            title: Some("Test Title".to_string()),
            link: Some("https://example.com/post".to_string()),
            date: Some("2024-01-01T00:00:00Z".to_string()),
            summary: Some("Test Summary".to_string()),
            author: Some("author@example.com (Author)".to_string()),
            ..Default::default()
        };

        let item = meta.into_rss_item().expect("Should convert to RSS item");
        assert_eq!(item.title(), Some("Test Title"));
        assert_eq!(item.link(), Some("https://example.com/post"));
        assert_eq!(item.description(), Some("Test Summary"));
        assert_eq!(item.author(), Some("author@example.com (Author)"));
        // RFC2822 format check
        assert!(item.pub_date().unwrap().contains("Jan 2024"));
    }

    #[test]
    fn test_post_meta_missing_fields() {
        let meta = PostMeta {
            title: None,
            link: Some("link".to_string()),
            date: Some("2024-01-01".to_string()),
            ..Default::default()
        };
        assert!(meta.into_rss_item().is_none());
    }

    #[test]
    fn test_parse_post_meta_basic() {
        let config = make_config("Author", "email@example.com");
        let json = r#"{
            "title": "Test Post",
            "date": "2024-01-01",
            "author": "Post Author"
        }"#;

        let meta = parse_post_meta("guid-123".to_string(), json, &config).unwrap();
        assert_eq!(meta.title, Some("Test Post".to_string()));
        assert_eq!(meta.link, Some("guid-123".to_string()));
        assert_eq!(meta.author, Some("email@example.com (Author)".to_string()));
        assert_eq!(meta.summary, None);
    }

    #[test]
    fn test_parse_post_meta_with_summary() {
        let config = make_config("Author", "email@example.com");
        // Construct a JSON string where "summary" is a string containing JSON
        // This matches the current implementation expectation:
        // get_string("summary") -> returns string -> parse_typst_element(string)
        let summary_inner = r#"{"func": "text", "text": "Summary Content"}"#;
        // Escape quotes for the outer JSON string
        let summary_escaped = summary_inner.replace('"', "\\\"");

        let json = format!(r#"{{
            "title": "Test Post",
            "date": "2024-01-01",
            "summary": "{}"
        }}"#, summary_escaped);

        let meta = parse_post_meta("guid".to_string(), &json, &config).unwrap();
        assert_eq!(meta.summary, Some("Summary Content".to_string()));
    }

    #[test]
    fn test_parse_post_meta_invalid_json() {
        let config = make_config("Author", "email@example.com");
        let json = r#"{"title": "Unclosed brace""#;
        assert!(parse_post_meta("guid".to_string(), json, &config).is_err());
    }
}


