//! Atom 1.0 feed generation.
//!
//! Generates Atom feeds from page metadata.

use super::common::{FeedPage, entry_body, feed_url, page_url};
use crate::{
    config::{FeedConfig, SiteConfig},
    core::UrlPath,
    utils::date::DateTimeUtc,
};
use anyhow::{Ok, Result};
use atom_syndication::{
    ContentBuilder, Entry, EntryBuilder, Feed, FeedBuilder, FixedDateTime, GeneratorBuilder, Link,
    LinkBuilder, Person, PersonBuilder, Text,
};

/// Render an Atom 1.0 feed.
pub(super) fn render(config: &SiteConfig, feed: &FeedConfig, pages: &[FeedPage]) -> Result<String> {
    AtomFeed {
        config,
        feed,
        pages,
    }
    .to_xml()
}

struct AtomFeed<'a> {
    config: &'a SiteConfig,
    feed: &'a FeedConfig,
    pages: &'a [FeedPage],
}

impl AtomFeed<'_> {
    fn to_xml(&self) -> Result<String> {
        let base_url = self.config.canonical_url(&UrlPath::from_page("/"));
        let feed_url = feed_url(self.config, self.feed);

        let entries: Vec<Entry> = self
            .pages
            .iter()
            .filter_map(|page| page_to_atom_entry(page, self.config, self.feed))
            .collect();

        // Find the most recent update time for feed updated field
        // Compare by RFC3339 strings (lexicographically sortable for ISO dates)
        let updated_str = self
            .pages
            .iter()
            .filter_map(|p| DateTimeUtc::parse(&p.date).map(|dt| dt.to_rfc3339()))
            .max()
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());

        let updated: FixedDateTime = updated_str
            .parse()
            .unwrap_or_else(|_| FixedDateTime::default());

        // Build author
        let author: Person = PersonBuilder::default()
            .name(self.config.site.info.author.clone())
            .email(Some(self.config.site.info.email.clone()))
            .build();

        // Build self link
        let self_link: Link = LinkBuilder::default()
            .href(feed_url)
            .rel("self".to_string())
            .mime_type(Some(self.feed.format.mime_type().to_string()))
            .build();

        // Build alternate link
        let alternate_link: Link = LinkBuilder::default()
            .href(base_url.clone())
            .rel("alternate".to_string())
            .build();

        let feed: Feed = FeedBuilder::default()
            .title(Text::plain(self.config.site.info.title.clone()))
            .id(base_url)
            .updated(updated)
            .authors(vec![author])
            .links(vec![self_link, alternate_link])
            .subtitle(Some(Text::plain(self.config.site.info.description.clone())))
            .generator(Some(
                GeneratorBuilder::default()
                    .value("tola-ssg")
                    .uri(Some("https://github.com/tola-rs/tola-ssg".to_string()))
                    .build(),
            ))
            .lang(self.config.site.info.language.clone())
            .entries(entries)
            .build();

        Ok(feed.to_string())
    }
}

fn page_to_atom_entry(page: &FeedPage, config: &SiteConfig, feed: &FeedConfig) -> Option<Entry> {
    let updated_str = DateTimeUtc::parse(&page.date)?.to_rfc3339();
    let updated: FixedDateTime = updated_str.parse().ok()?;

    let link = page_url(page, config);

    // Build entry link
    let entry_link: Link = LinkBuilder::default()
        .href(&link)
        .rel("alternate".to_string())
        .build();

    // Build author if available
    let authors: Vec<Person> = page
        .author
        .as_ref()
        .map(|name| vec![PersonBuilder::default().name(name.clone()).build()])
        .unwrap_or_default();

    let content = entry_body(page, config, feed).map(|html| {
        ContentBuilder::default()
            .value(html)
            .content_type("html".to_string())
            .build()
    });

    Some(
        EntryBuilder::default()
            .title(Text::plain(page.title.clone()))
            .id(&link)
            .updated(updated)
            .links(vec![entry_link])
            .summary(
                page.summary
                    .as_ref()
                    .map(|summary| Text::plain(&summary.text)),
            )
            .content(content)
            .authors(authors)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FeedFeature, FeedFormat};
    use crate::seo::feed::common::FeedSummary;

    // Helper to create a config for testing
    fn make_config() -> SiteConfig {
        let mut config = SiteConfig::default();
        config.site.info.title = "Test Blog".to_string();
        config.site.info.author = "Test Author".to_string();
        config.site.info.email = "test@example.com".to_string();
        config.site.info.url = Some("https://example.com".to_string());
        config.site.info.description = "A test blog".to_string();
        config
    }

    fn feed_config(features: Vec<FeedFeature>) -> FeedConfig {
        FeedConfig {
            format: FeedFormat::Atom,
            output: "atom.xml".into(),
            features,
        }
    }

    #[test]
    fn test_page_to_atom_entry_basic() {
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
            author: Some("Post Author".to_string()),
        };

        let feed = feed_config(vec![]);
        let entry = page_to_atom_entry(&page, &config, &feed).expect("should create entry");
        assert_eq!(entry.title().as_str(), "Test Post");
        assert_eq!(entry.id(), "https://example.com/test/");
        assert!(entry.updated().to_rfc3339().starts_with("2024-01-15"));
        assert_eq!(entry.content(), None);
    }

    #[test]
    fn test_page_to_atom_entry_full_text() {
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
            author: Some("Post Author".to_string()),
        };

        let feed = feed_config(vec![FeedFeature::FullText]);
        let entry = page_to_atom_entry(&page, &config, &feed).expect("should create entry");
        assert_eq!(entry.summary().map(Text::as_str), Some("A test summary"));
        let content = entry.content().expect("should include full text");
        assert_eq!(content.content_type(), Some("html"));
        assert_eq!(content.value(), Some("<article><p>Full text</p></article>"));
    }

    #[test]
    fn test_page_to_atom_entry_invalid_date() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "invalid-date".to_string(),
            permalink: "/test/".to_string(),
            summary: None,
            feed_body: None,
            author: None,
        };

        // Invalid date should return None
        let feed = feed_config(vec![]);
        assert!(page_to_atom_entry(&page, &config, &feed).is_none());
    }
}
