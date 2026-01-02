//! Atom 1.0 feed generation.
//!
//! Generates Atom feeds from page metadata stored in STORED_PAGES.

use super::common::{FeedPage, get_feed_pages};
use crate::{config::SiteConfig, generator::minify_xml, log, utils::date::DateTimeUtc};
use anyhow::{Ok, Result};
use atom_syndication::{
    Entry, EntryBuilder, Feed, FeedBuilder, FixedDateTime, GeneratorBuilder, Link, LinkBuilder,
    Person, PersonBuilder, Text,
};
use std::fs;

/// Build Atom 1.0 feed.
pub fn build_atom(config: &SiteConfig) -> Result<()> {
    AtomFeed::build(config).write()
}

struct AtomFeed {
    config: SiteConfig,
    pages: Vec<FeedPage>,
}

impl AtomFeed {
    fn build(config: &SiteConfig) -> Self {
        let pages = get_feed_pages();
        Self {
            config: config.clone(),
            pages,
        }
    }

    fn into_xml(self) -> Result<String> {
        let base_url = self
            .config
            .site
            .info
            .url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');

        let entries: Vec<Entry> = self
            .pages
            .iter()
            .filter_map(|page| page_to_atom_entry(page, &self.config))
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
            .href(format!(
                "{}/{}",
                base_url,
                self.config.build.feed.path.display()
            ))
            .rel("self".to_string())
            .mime_type(Some("application/atom+xml".to_string()))
            .build();

        // Build alternate link
        let alternate_link: Link = LinkBuilder::default()
            .href(base_url.to_string())
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
                    .uri(Some("https://github.com/kawayww/tola-ssg".to_string()))
                    .build(),
            ))
            .lang(self.config.site.info.language.clone())
            .entries(entries)
            .build();

        Ok(feed.to_string())
    }

    fn write(self) -> Result<()> {
        let minify = self.config.build.minify;
        let output_dir = self.config.paths().output_dir();
        let feed_path = self.config.build.feed.path.clone();
        let xml = self.into_xml()?;
        let xml = minify_xml(xml.as_bytes(), minify);
        // Resolve feed path relative to output_dir (with path_prefix)
        let atom_path = output_dir.join(&feed_path);

        if let Some(parent) = atom_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&atom_path, &*xml)?;

        log!("atom"; "{}", atom_path.file_name().unwrap_or_default().to_string_lossy());
        Ok(())
    }
}

fn page_to_atom_entry(page: &FeedPage, config: &SiteConfig) -> Option<Entry> {
    let updated_str = DateTimeUtc::parse(&page.date)?.to_rfc3339();
    let updated: FixedDateTime = updated_str.parse().ok()?;

    // Build full URL from base URL + permalink
    let base_url = config
        .site
        .info
        .url
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches('/');
    let link = format!("{}{}", base_url, page.permalink);

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

    Some(
        EntryBuilder::default()
            .title(Text::plain(page.title.clone()))
            .id(&link)
            .updated(updated)
            .links(vec![entry_link])
            .summary(page.summary.clone().map(Text::plain))
            .authors(authors)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_page_to_atom_entry_basic() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: Some("A test summary".to_string()),
            author: Some("Post Author".to_string()),
        };

        let entry = page_to_atom_entry(&page, &config).expect("should create entry");
        assert_eq!(entry.title().as_str(), "Test Post");
        assert_eq!(entry.id(), "https://example.com/test/");
        assert!(entry.updated().to_rfc3339().starts_with("2024-01-15"));
    }

    #[test]
    fn test_page_to_atom_entry_invalid_date() {
        let config = make_config();
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "invalid-date".to_string(),
            permalink: "/test/".to_string(),
            summary: None,
            author: None,
        };

        // Invalid date should return None
        assert!(page_to_atom_entry(&page, &config).is_none());
    }
}
