//! Draft filtering for Markdown files.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::convert::MarkdownMetaExtractor;
use crate::compiler::page::Markdown;
use crate::compiler::page::format::{DraftFilter, FilterResult, ScannedHeading, ScannedPage};
use crate::page::{PageKind, PageMeta};

/// Result of Markdown draft filtering with scanned data.
pub struct MarkdownFilterResult {
    /// Pre-scanned page data for non-draft files.
    pub scanned: Vec<ScannedPage>,
    /// Number of draft files filtered out.
    pub draft_count: usize,
}

/// Filter Markdown files, removing drafts.
///
/// Also collects metadata and extracts links for pre-scan optimization.
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

/// Extract PageMeta from Markdown content.
fn extract_meta(content: &str) -> Option<PageMeta> {
    MarkdownMetaExtractor
        .extract_frontmatter(content)
        .ok()
        .flatten()
        .map(|(meta, _)| meta)
}

/// Extract internal page links from Markdown content.
fn extract_links(content: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Parser, Tag};

    let parser = Parser::new(content);
    let mut links = Vec::new();

    for event in parser {
        if let Event::Start(Tag::Link { dest_url, .. }) = event {
            let url = dest_url.as_ref();
            // Only collect site-root links (starting with /)
            if url.starts_with('/') && !url.starts_with("//") {
                links.push(url.to_string());
            }
        }
    }

    links
}

/// Extract headings from Markdown content.
fn extract_headings(content: &str) -> Vec<ScannedHeading> {
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

impl DraftFilter for Markdown {
    type Extra = ();

    fn filter_drafts<'a>(
        files: Vec<&'a PathBuf>,
        root: &'a Path,
        label: &str,
    ) -> FilterResult<'a, Self::Extra> {
        let result = filter_drafts(&files, root, label);
        let non_draft_files: Vec<_> = result
            .scanned
            .iter()
            .filter_map(|s| files.iter().find(|f| ***f == s.path).copied())
            .collect();
        FilterResult::new(non_draft_files, result.draft_count)
    }
}
