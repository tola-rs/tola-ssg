//! RSS 2.0 serialization with independent entry identity and body.

use anyhow::{Result, anyhow};
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder, validation::Validate};

use super::declaration::{Feed, FeedEntry};
use crate::cancellation::BuildCancellation;

pub(super) fn render(feed: &Feed, cancellation: &BuildCancellation) -> Result<String> {
    cancellation.ensure_active()?;
    let entries = feed
        .entries
        .iter()
        .map(|value| entry(value, cancellation))
        .collect::<Result<Vec<_>>>()?;
    let mut namespaces = std::collections::BTreeMap::new();
    if feed.entries.iter().any(|entry| entry.content.is_some()) {
        namespaces.insert(
            "content".into(),
            "http://purl.org/rss/1.0/modules/content/".into(),
        );
    }
    let channel = ChannelBuilder::default()
        .title(feed.title.clone())
        .link(feed.home_url.clone())
        .description(feed.description.clone())
        .language(feed.language.clone())
        .generator("tola-ssg".to_owned())
        .namespaces(namespaces)
        .items(entries)
        .build();
    channel.validate().map_err(|_| {
        anyhow!("RSS does not accept this feed; check the entry links, ids, and dates")
    })?;
    super::write_serialized(cancellation, |output| {
        channel.write_to(output)?;
        Ok(())
    })
}

fn entry(entry: &FeedEntry, cancellation: &BuildCancellation) -> Result<rss::Item> {
    cancellation.ensure_active()?;
    let mut author = None;
    for candidate in &entry.authors {
        cancellation.ensure_active()?;
        if let Some(email) = &candidate.email {
            author = Some(format!("{email} ({})", candidate.name));
            break;
        }
    }
    Ok(ItemBuilder::default()
        .title(entry.title.clone())
        .link(entry.url.clone())
        .guid(
            GuidBuilder::default()
                .permalink(false)
                .value(entry.id.clone())
                .build(),
        )
        .description(
            entry
                .summary
                .as_ref()
                .map(|summary| summary.html(&entry.url, cancellation))
                .transpose()?,
        )
        .content(
            entry
                .content
                .as_ref()
                .map(|content| content.html(&entry.url, cancellation))
                .transpose()?,
        )
        .pub_date(entry.published.to_rfc2822())
        .author(author)
        .build())
}
