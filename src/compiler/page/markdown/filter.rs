//! Draft filtering for Markdown files.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::convert::MarkdownMetaExtractor;
use crate::compiler::page::format::{ScannedHeading, ScannedPage, ScannedPageLink};
use crate::core::{LinkKind, LinkOrigin};
use crate::page::{PageKind, PageMeta};

/// Result of Markdown draft filtering with scanned data
pub struct MarkdownFilterResult {
    /// Pre-scanned page data for non-draft files.
    pub scanned: Vec<ScannedPage>,
    /// Number of draft files filtered out.
    pub draft_count: usize,
}

/// Filter Markdown files, removing drafts
///
/// Also collects metadata and extracts links for pre-scan optimization
pub fn filter_drafts(files: &[&PathBuf], _root: &Path, _label: &str) -> MarkdownFilterResult {
    let results: Vec<_> = files
        .par_iter()
        .filter_map(|path| {
            let content = std::fs::read_to_string(path).ok()?;
            let meta = extract_meta(&content);
            let links = extract_links(&content);
            let headings = extract_headings(&content);
            Some(((*path).clone(), meta, links, headings))
        })
        .collect();

    let mut scanned = Vec::new();
    let mut draft_count = 0;

    for (path, meta, links, headings) in results {
        if meta.as_ref().map(|m| m.draft).unwrap_or(false) {
            draft_count += 1;
        } else {
            scanned.push(ScannedPage {
                path,
                meta,
                kind: PageKind::Direct, // Markdown never imports @tola/*
                links,
                headings,
            });
        }
    }

    MarkdownFilterResult {
        scanned,
        draft_count,
    }
}

/// Extract PageMeta from Markdown content
fn extract_meta(content: &str) -> Option<PageMeta> {
    MarkdownMetaExtractor
        .extract_frontmatter(content)
        .ok()
        .flatten()
        .map(|(meta, _)| meta)
}

/// Extract internal page links from Markdown content
pub fn extract_links(content: &str) -> Vec<ScannedPageLink> {
    use pulldown_cmark::{Event, Parser, Tag};

    let parser = Parser::new(content);
    let mut links = Vec::new();

    for event in parser {
        if let Event::Start(Tag::Link { dest_url, .. }) = event {
            let url = dest_url.as_ref();
            let link = ScannedPageLink::new(url, LinkOrigin::Href);
            if link.is_page_candidate() && !matches!(LinkKind::parse(url), LinkKind::Fragment(_)) {
                links.push(link);
            }
        }
    }

    links
}

/// Extract headings from Markdown content
pub fn extract_headings(content: &str) -> Vec<ScannedHeading> {
    use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

    let parser = Parser::new(content);
    let mut headings = Vec::new();
    let mut current_heading: Option<(u8, String)> = None;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let level_num = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                current_heading = Some((level_num, String::new()));
            }
            Event::Text(text) if current_heading.is_some() => {
                if let Some((_, ref mut content)) = current_heading {
                    content.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = current_heading.take() {
                    headings.push(ScannedHeading {
                        level,
                        text,
                        supplement: None,
                    });
                }
            }
            _ => {}
        }
    }

    headings
}

#[cfg(test)]
mod tests {
    use super::extract_links;

    #[test]
    fn test_extract_links_keeps_relative_page_links_and_skips_images() {
        let content = r#"
[About](../about/)
[Home](/)
![Cat](./cat.png)
"#;

        let links = extract_links(content);

        assert!(links.iter().any(|link| link.dest == "../about/"));
        assert!(links.iter().any(|link| link.dest == "/"));
        assert!(!links.iter().any(|link| link.dest == "./cat.png"));
    }
}
