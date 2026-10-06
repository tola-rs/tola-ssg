//! Atom 1.0 serialization of explicit channel and entry values.

use anyhow::Result;
use atom_syndication::{
    ContentBuilder, EntryBuilder, FeedBuilder, GeneratorBuilder, LinkBuilder, PersonBuilder, Text,
};

use super::declaration::{EntryContent, EntryText, Feed, FeedEntry};
use crate::cancellation::{BuildCancellation, BuildCancelled};

pub(super) fn render(feed: &Feed, cancellation: &BuildCancellation) -> Result<String> {
    cancellation.ensure_active()?;
    let mut updated: Option<atom_syndication::FixedDateTime> = None;
    for entry in &feed.entries {
        cancellation.ensure_active()?;
        let date = entry.updated.unwrap_or(entry.published);
        updated = Some(updated.map_or(date, |current| current.max(date)));
    }
    let updated = updated.unwrap_or_else(|| "1970-01-01T00:00:00Z".parse().expect("fixed epoch"));
    let authors = super::map_authors(&feed.authors, cancellation, author)?;
    let links = vec![
        LinkBuilder::default()
            .href(feed.url.clone())
            .rel("self")
            .mime_type(Some(feed.format.mime_type().to_owned()))
            .build(),
        LinkBuilder::default()
            .href(feed.home_url.clone())
            .rel("alternate")
            .build(),
    ];
    let document = FeedBuilder::default()
        .title(Text::plain(feed.title.clone()))
        .id(feed.id.clone())
        .updated(updated)
        .authors(authors)
        .links(links)
        .subtitle(Some(Text::plain(feed.description.clone())))
        .generator(Some(GeneratorBuilder::default().value("tola-ssg").build()))
        .lang(feed.language.clone())
        .entries(
            feed.entries
                .iter()
                .map(|value| entry(value, cancellation))
                .collect::<Result<Vec<_>>>()?,
        )
        .build();
    super::write_serialized(cancellation, |output| {
        document.write_to(output)?;
        Ok(())
    })
}

fn entry(entry: &FeedEntry, cancellation: &BuildCancellation) -> Result<atom_syndication::Entry> {
    cancellation.ensure_active()?;
    let content = entry
        .content
        .as_ref()
        .map(|content| {
            let (kind, body) = match content {
                EntryContent::Authored(EntryText::Text(text)) => ("text", text.clone()),
                _ => ("html", content.html(&entry.url, cancellation)?),
            };
            Ok::<_, BuildCancelled>(
                ContentBuilder::default()
                    .content_type(Some(kind.to_owned()))
                    .value(Some(body))
                    .build(),
            )
        })
        .transpose()?;
    let summary = entry
        .summary
        .as_ref()
        .map(|summary| {
            Ok::<_, BuildCancelled>(match summary {
                EntryText::Text(text) => Text::plain(text.clone()),
                EntryText::Rich(_) => Text::html(summary.html(&entry.url, cancellation)?),
            })
        })
        .transpose()?;
    let authors = super::map_authors(&entry.authors, cancellation, author)?;
    Ok(EntryBuilder::default()
        .title(Text::plain(entry.title.clone()))
        .id(entry.id.clone())
        .published(Some(entry.published))
        .updated(entry.updated.unwrap_or(entry.published))
        .links(vec![
            LinkBuilder::default()
                .href(entry.url.clone())
                .rel("alternate")
                .build(),
        ])
        .summary(summary)
        .content(content)
        .authors(authors)
        .build())
}

fn author(author: &super::declaration::Author) -> atom_syndication::Person {
    PersonBuilder::default()
        .name(author.name.clone())
        .email(author.email.clone())
        .uri(author.url.clone())
        .build()
}
