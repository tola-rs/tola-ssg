//! The document projection of one help page: its Markdown plus the headings, links, and code
//! blocks a program reads without parsing it.

use anyhow::{Result, bail};
use pulldown_cmark::{Event, Options, Parser, Tag};
use serde::Serialize;

use super::model::{Block, HelpDocument, HelpPage, Inline, LinkTarget, plain_text};

#[derive(Serialize)]
pub(crate) struct Heading {
    pub level: u8,
    pub title: String,
    pub anchor: String,
}

#[derive(Serialize)]
pub(crate) struct Link {
    pub label: String,
    pub target: String,
}

#[derive(Serialize)]
pub(crate) struct Document {
    pub target: String,
    pub markdown: String,
    pub headings: Vec<Heading>,
    pub links: Vec<Link>,
    pub code_blocks: Vec<String>,
}

fn links(spans: &[Inline], result: &mut Vec<Link>) {
    for span in spans {
        match span {
            Inline::Link { label, target } => {
                result.push(Link {
                    label: plain_text(label),
                    target: target.uri(),
                });
                links(label, result);
            }
            Inline::Emph(inner) | Inline::Strong(inner) => links(inner, result),
            _ => {}
        }
    }
}

/// The semantic indexes one page's blocks contribute.
#[derive(Default)]
struct PageIndex {
    headings: Vec<Heading>,
    links: Vec<Link>,
    code_blocks: Vec<String>,
}

impl PageIndex {
    fn collect(&mut self, blocks: &[Block]) {
        for block in blocks {
            match block {
                Block::Heading {
                    level,
                    spans,
                    anchor,
                    ..
                } => {
                    self.headings.push(Heading {
                        level: *level,
                        title: plain_text(spans),
                        anchor: anchor.as_str().to_owned(),
                    });
                    links(spans, &mut self.links);
                }
                Block::Paragraph { spans, .. } => links(spans, &mut self.links),
                Block::List { items, .. } => {
                    for item in items {
                        self.collect(item);
                    }
                }
                Block::Quote { blocks } => self.collect(blocks),
                Block::Code { source, .. } => self.code_blocks.push(source.clone()),
                Block::Table { headers, rows } => {
                    for cell in headers.iter().chain(rows.iter().flatten()) {
                        links(cell, &mut self.links);
                    }
                }
                Block::Rule => {}
            }
        }
    }
}

/// Return original Markdown and semantic indexes; no terminal rows or styles enter this output.
pub(crate) fn document(page: &HelpPage, target: &LinkTarget) -> Result<Document> {
    let parsed = HelpDocument::parse(page);
    let mut page_index = PageIndex::default();
    page_index.collect(&parsed.blocks);
    let markdown = page.markdown();
    let markdown = match target {
        LinkTarget::PageAnchor(_, anchor) => {
            let Some(position) = page_index
                .headings
                .iter()
                .position(|heading| heading.anchor == anchor.as_str())
            else {
                bail!("anchor {:?} is not on this help page", anchor.as_str());
            };
            let offsets = Parser::new_ext(&markdown, Options::ENABLE_TABLES)
                .into_offset_iter()
                .filter_map(|(event, range)| {
                    matches!(event, Event::Start(Tag::Heading { .. })).then_some(range.start)
                })
                .collect::<Vec<_>>();
            let start = *offsets
                .get(position)
                .ok_or_else(|| anyhow::anyhow!("help heading has no source position"))?;
            let next = page_index
                .headings
                .iter()
                .enumerate()
                .skip(position + 1)
                .find(|(_, heading)| heading.level <= page_index.headings[position].level)
                .map_or(page_index.headings.len(), |(next, _)| next);
            let end = offsets.get(next).copied().unwrap_or(markdown.len());
            let selected = markdown[start..end].to_owned();
            page_index.headings.truncate(next);
            page_index.headings.drain(..position);
            let section = HelpDocument::parse(&HelpPage::new(page.id.clone(), selected.clone()));
            // The selected heading is already in `headings`; this pass supplies links and code only.
            let mut section_index = PageIndex::default();
            section_index.collect(&section.blocks);
            page_index.links = section_index.links;
            page_index.code_blocks = section_index.code_blocks;
            selected
        }
        _ => markdown,
    };
    Ok(Document {
        target: target.uri(),
        markdown,
        headings: page_index.headings,
        links: page_index.links,
        code_blocks: page_index.code_blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::model::{PageId, anchor};

    #[test]
    fn export_anchors_source_sections_identity() {
        let mut page = HelpPage::new(
            PageId::Overview,
            "# Intro\n\n## test - function\n\nProse\n".into(),
        );
        page.push_export(
            "\n## test - function\n\n```typ\n#test()\n```\n\n[other](tola-help://config)\n".into(),
        );
        page.push_export(
            "\n## another\n\n```typ\n#another()\n```\n\n[extra](tola-help://packages)\n".into(),
        );
        let target = LinkTarget::PageAnchor(PageId::Overview, anchor("test - function"));
        let document = document(&page, &target).unwrap();
        assert!(document.markdown.contains("#test()"));
        assert!(!document.markdown.contains("Prose"));
        assert_eq!(document.code_blocks, ["#test()\n"]);
        assert_eq!(document.headings.len(), 1);
        assert_eq!(document.headings[0].anchor, "test-function");
        assert_eq!(document.links.len(), 1);
        assert_eq!(document.links[0].target, "tola-help://config");
    }
}
