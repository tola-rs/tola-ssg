//! JSON Feed 1.1 serialization.

use anyhow::Result;
use serde::Serialize;

use super::declaration::{EntryContent, EntryText, Feed, FeedEntry};
use crate::cancellation::BuildCancellation;

pub(super) fn render(feed: &Feed, cancellation: &BuildCancellation) -> Result<String> {
    cancellation.ensure_active()?;
    let document = JsonFeed {
        version: "https://jsonfeed.org/version/1.1",
        title: &feed.title,
        home_page_url: &feed.home_url,
        feed_url: &feed.url,
        description: &feed.description,
        language: &feed.language,
        authors: super::map_authors(&feed.authors, cancellation, author)?,
        items: feed
            .entries
            .iter()
            .map(|value| entry(value, cancellation))
            .collect::<Result<Vec<_>>>()?,
    };
    super::write_serialized(cancellation, |output| {
        Ok(serde_json::to_writer_pretty(output, &document)?)
    })
}

#[derive(Serialize)]
struct JsonFeed<'a> {
    version: &'static str,
    title: &'a str,
    home_page_url: &'a str,
    feed_url: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    language: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    authors: Vec<JsonAuthor<'a>>,
    items: Vec<JsonEntry<'a>>,
}

#[derive(Serialize)]
struct JsonAuthor<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonEntry<'a> {
    id: &'a str,
    url: &'a str,
    title: &'a str,
    date_published: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    date_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    authors: Vec<JsonAuthor<'a>>,
}

fn entry<'a>(entry: &'a FeedEntry, cancellation: &BuildCancellation) -> Result<JsonEntry<'a>> {
    cancellation.ensure_active()?;
    let (content_html, content_text) = match &entry.content {
        None => (None, Some(String::new())),
        Some(EntryContent::Authored(EntryText::Text(text))) => (None, Some(text.clone())),
        Some(content) => (Some(content.html(&entry.url, cancellation)?), None),
    };
    Ok(JsonEntry {
        id: &entry.id,
        url: &entry.url,
        title: &entry.title,
        date_published: entry.published.to_rfc3339(),
        date_modified: entry.updated.map(|date| date.to_rfc3339()),
        summary: entry
            .summary
            .as_ref()
            .map(|value| value.plain_text(cancellation))
            .transpose()?,
        content_html,
        content_text,
        authors: super::map_authors(&entry.authors, cancellation, author)?,
    })
}

fn author(author: &super::declaration::Author) -> JsonAuthor<'_> {
    JsonAuthor {
        name: &author.name,
        url: author.url.as_deref(),
    }
}
