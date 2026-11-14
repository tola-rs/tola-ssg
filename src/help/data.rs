//! Reference data produced by the help page builders.

use anyhow::{Result, bail};
use pulldown_cmark::{Event, Options, Parser, Tag};
use serde::{Serialize, Serializer};

use super::model::{Block, HelpDocument, HelpPage, Inline, LinkTarget, plain_text};

struct Heading {
    level: u8,
    anchor: String,
}

pub(super) fn serialize_target<S: Serializer>(
    target: &LinkTarget,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&target.uri())
}

pub(crate) struct Document {
    pub markdown: String,
    pub links: Vec<LinkTarget>,
}

#[derive(Serialize)]
pub(super) struct ReferenceLink {
    pub title: String,
    #[serde(serialize_with = "serialize_target")]
    pub target: LinkTarget,
}

#[derive(Default, Serialize)]
pub(crate) struct Reference {
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Block>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) entries: Vec<ReferenceLink>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) exports: Vec<Export>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) commands: Vec<Command>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) keys: Vec<Key>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) related: Vec<ReferenceLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Export {
    pub name: String,
    pub kind: &'static str,
    pub anchor: String,
    #[serde(flatten)]
    pub signature: Signature,
    pub blocks: Vec<Block>,
    pub methods: Vec<Method>,
    pub related: Vec<ReferenceLink>,
}

#[derive(Serialize)]
pub(super) struct Signature {
    pub declaration: String,
    pub parameters: Vec<Parameter>,
    pub returns: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Method {
    pub name: String,
    pub anchor: String,
    #[serde(flatten)]
    pub signature: Signature,
    pub blocks: Vec<Block>,
}

#[derive(Serialize)]
pub(super) struct Command {
    pub name: String,
    pub anchor: String,
    pub usage: String,
    pub blocks: Vec<Block>,
    pub parameters: Vec<Parameter>,
    pub options: Vec<Parameter>,
}

#[derive(Serialize)]
pub(super) struct Parameter {
    pub name: String,
    pub kind: &'static str,
    pub ty: Option<String>,
    pub required: bool,
    pub default: Option<String>,
    pub blocks: Vec<Block>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<char>,
}

#[derive(Serialize)]
pub(super) struct Key {
    pub path: String,
    pub blocks: Vec<Block>,
}

#[derive(Serialize)]
pub(crate) struct ReferenceDocument {
    target: String,
    page: String,
    language: &'static str,
    #[serde(flatten)]
    reference: Reference,
}

pub(crate) fn reference(
    mut page: HelpPage,
    target: &LinkTarget,
    language: crate::i18n::HelpLanguage,
) -> Result<ReferenceDocument> {
    if page.reference.title.is_empty() {
        let parsed = HelpDocument::parse(&page);
        let mut blocks = parsed.blocks.into_iter();
        if let Some(Block::Heading { spans, .. }) = blocks.next() {
            page.reference.title = plain_text(&spans);
        }
        if page.reference.entries.is_empty() {
            page.reference.blocks = blocks.collect();
        }
    }
    if let LinkTarget::PageAnchor(_, anchor) = target {
        if page
            .reference
            .commands
            .iter()
            .any(|command| command.anchor == anchor.as_str())
        {
            page.reference
                .commands
                .retain(|command| command.anchor == anchor.as_str());
            page.reference.blocks.clear();
        } else if page
            .reference
            .exports
            .iter()
            .any(|export| export.anchor == anchor.as_str())
        {
            page.reference
                .exports
                .retain(|export| export.anchor == anchor.as_str());
            page.reference.blocks.clear();
        } else {
            let selected = document(&page, target)?;
            page.reference.blocks = page.prose(&selected.markdown);
            page.reference.entries.clear();
            page.reference.exports.clear();
            page.reference.commands.clear();
            page.reference.related.clear();
            page.reference.keys.clear();
            page.reference.import = None;
        }
    }
    Ok(ReferenceDocument {
        target: target.uri(),
        page: page.id.canonical_uri(),
        language: language.tag(),
        reference: page.reference,
    })
}

fn links(spans: &[Inline], result: &mut Vec<LinkTarget>) {
    for span in spans {
        match span {
            Inline::Link { label, target } => {
                result.push(target.clone());
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
    links: Vec<LinkTarget>,
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
                Block::Table { headers, rows, .. } => {
                    for cell in headers.iter().chain(rows.iter().flatten()) {
                        links(cell, &mut self.links);
                    }
                }
                Block::Code { .. } | Block::Rule => {}
            }
        }
    }
}

/// Select the Markdown and links used by the terminal help browser.
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
            let section = HelpDocument::parse(&HelpPage::new(page.id.clone(), selected.clone()));
            // The selected heading is already in `headings`; this pass supplies links and code only.
            let mut section_index = PageIndex::default();
            section_index.collect(&section.blocks);
            page_index.links = section_index.links;
            selected
        }
        _ => markdown,
    };
    Ok(Document {
        markdown,
        links: page_index.links,
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
        assert_eq!(document.links.len(), 1);
        assert_eq!(document.links[0].uri(), "tola-help://config");
    }
}
