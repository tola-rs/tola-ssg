//! JSON Feed 1.1 generation.

use super::common::{FeedPage, entry_content, feed_url, page_url};
use crate::{
    config::{FeedConfig, SiteConfig},
    core::UrlPath,
    utils::date::DateTimeUtc,
};
use anyhow::Result;
use serde::Serialize;

const JSON_FEED_VERSION: &str = "https://jsonfeed.org/version/1.1";

/// Render a JSON Feed 1.1 feed.
pub(super) fn render(config: &SiteConfig, feed: &FeedConfig, pages: &[FeedPage]) -> Result<String> {
    JsonFeed {
        config,
        feed,
        pages,
    }
    .render()
}

struct JsonFeed<'a> {
    config: &'a SiteConfig,
    feed: &'a FeedConfig,
    pages: &'a [FeedPage],
}

impl JsonFeed<'_> {
    fn render(&self) -> Result<String> {
        let home_page_url = self.config.canonical_url(&UrlPath::from_page("/"));
        let feed_url = feed_url(self.config, self.feed);

        let authors = author_list(&self.config.site.info.author);
        let items = self
            .pages
            .iter()
            .filter_map(|page| page_to_item(page, self.config, self.feed))
            .collect();

        let feed = Document {
            version: JSON_FEED_VERSION,
            title: &self.config.site.info.title,
            home_page_url,
            feed_url,
            description: non_empty(&self.config.site.info.description),
            language: non_empty(&self.config.site.info.language),
            authors,
            items,
        };

        Ok(serde_json::to_string_pretty(&feed)?)
    }
}

#[derive(Debug, Serialize)]
struct Document<'a> {
    version: &'static str,
    title: &'a str,
    home_page_url: String,
    feed_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    authors: Vec<Author>,
    items: Vec<Item>,
}

#[derive(Debug, Serialize)]
struct Author {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Serialize)]
struct Item {
    id: String,
    url: String,
    title: String,
    date_published: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    authors: Vec<Author>,
}

fn page_to_item(page: &FeedPage, config: &SiteConfig, feed: &FeedConfig) -> Option<Item> {
    let date_published = DateTimeUtc::parse(&page.date)?.to_rfc3339();

    let url = page_url(page, config);
    let summary = page.summary.as_ref().map(|summary| summary.text.clone());
    let content_html = entry_content(page, config, feed);
    let content_text = content_html.is_none().then(String::new);

    Some(Item {
        id: url.clone(),
        url,
        title: page.title.clone(),
        date_published,
        summary,
        content_html,
        content_text,
        authors: page
            .author
            .as_deref()
            .and_then(author)
            .into_iter()
            .collect(),
    })
}

fn author_list(name: &str) -> Vec<Author> {
    author(name).into_iter().collect()
}

fn author(name: &str) -> Option<Author> {
    let name = non_empty(name).map(ToOwned::to_owned);

    name.is_some().then_some(Author { name })
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FeedFeature, FeedFormat};
    use crate::seo::feed::common::FeedSummary;

    fn make_config() -> SiteConfig {
        let mut config = SiteConfig::default();
        config.site.info.title = "Test Blog".to_string();
        config.site.info.author = "Test Author".to_string();
        config.site.info.email = "test@example.com".to_string();
        config.site.info.url = Some("https://example.com".to_string());
        config.site.info.description = "A test blog".to_string();
        config.site.info.language = "en".to_string();
        config
    }

    fn feed_config(features: Vec<FeedFeature>) -> FeedConfig {
        FeedConfig {
            format: FeedFormat::Json,
            output: "feed.json".into(),
            features,
        }
    }

    #[test]
    fn page_to_item_uses_summary_when_full_text_is_disabled() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: Some(FeedSummary {
                html: r#"A <a href="https://example.com">test</a> summary"#.to_string(),
                text: "A test summary".to_string(),
            }),
            feed_body: Some("<article><p>Full text</p></article>".to_string()),
            author: Some("Post Author".to_string()),
        };

        let feed = feed_config(vec![]);
        let item = page_to_item(&page, &config, &feed).expect("should create item");

        assert_eq!(item.id, "https://example.com/test/");
        assert_eq!(item.url, "https://example.com/test/");
        assert_eq!(item.date_published, "2024-01-15T00:00:00Z");
        assert_eq!(item.summary.as_deref(), Some("A test summary"));
        assert_eq!(
            item.content_html.as_deref(),
            Some(r#"A <a href="https://example.com">test</a> summary"#)
        );
        assert_eq!(item.content_text, None);
        assert_eq!(item.authors.len(), 1);
        assert_eq!(item.authors[0].name.as_deref(), Some("Post Author"));
    }

    #[test]
    fn page_to_item_uses_feed_body_when_full_text_is_enabled() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: Some(FeedSummary {
                html: "A test summary".to_string(),
                text: "A test summary".to_string(),
            }),
            feed_body: Some("<article><p>Full text</p></article>".to_string()),
            author: None,
        };

        let feed = feed_config(vec![FeedFeature::FullText]);
        let item = page_to_item(&page, &config, &feed).expect("should create item");

        assert_eq!(item.summary.as_deref(), Some("A test summary"));
        assert_eq!(
            item.content_html.as_deref(),
            Some("<article><p>Full text</p></article>")
        );
        assert_eq!(item.content_text, None);
    }

    #[test]
    fn page_to_item_falls_back_to_summary_when_full_text_body_is_missing() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: Some(FeedSummary {
                html: "A test summary".to_string(),
                text: "A test summary".to_string(),
            }),
            feed_body: None,
            author: None,
        };

        let feed = feed_config(vec![FeedFeature::FullText]);
        let item = page_to_item(&page, &config, &feed).expect("should create item");

        assert_eq!(item.summary.as_deref(), Some("A test summary"));
        assert_eq!(item.content_html.as_deref(), Some("A test summary"));
        assert_eq!(item.content_text, None);
    }

    #[test]
    fn page_to_item_falls_back_to_empty_text_content() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: None,
            feed_body: None,
            author: None,
        };

        let feed = feed_config(vec![FeedFeature::FullText]);
        let item = page_to_item(&page, &config, &feed).expect("should create item");

        assert_eq!(item.content_html, None);
        assert_eq!(item.content_text.as_deref(), Some(""));
    }

    #[test]
    fn page_to_item_rejects_invalid_date() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "invalid-date".to_string(),
            permalink: "/test/".to_string(),
            summary: None,
            feed_body: None,
            author: None,
        };

        let feed = feed_config(vec![]);
        assert!(page_to_item(&page, &config, &feed).is_none());
    }
}
