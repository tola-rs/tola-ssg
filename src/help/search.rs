//! Plain text grouped by the help heading that opens it.

use serde::Serialize;

use super::model::{Block, HelpDocument, HelpPage, LinkTarget, plain_text};

#[derive(Serialize)]
pub(crate) struct Section {
    page: String,
    section: String,
    target: String,
    paragraphs: Vec<String>,
}

pub(crate) fn sections(page: &HelpPage, title: &str) -> Vec<Section> {
    let document = HelpDocument::parse(page);
    let mut sections = vec![Section {
        page: title.to_owned(),
        section: String::new(),
        target: page.id.uri(),
        paragraphs: Vec::new(),
    }];
    let blocks = match document.blocks.as_slice() {
        [Block::Heading { level: 1, .. }, rest @ ..] => rest,
        blocks => blocks,
    };
    collect(blocks, page, title, &mut sections);
    sections
}

fn collect(blocks: &[Block], page: &HelpPage, title: &str, sections: &mut Vec<Section>) {
    for block in blocks {
        let text = match block {
            Block::Heading { spans, anchor, .. } => {
                sections.push(Section {
                    page: title.to_owned(),
                    section: plain_text(spans),
                    target: LinkTarget::PageAnchor(page.id.clone(), anchor.clone()).uri(),
                    paragraphs: Vec::new(),
                });
                continue;
            }
            Block::Paragraph { spans } => plain_text(spans),
            Block::Code { source } => source.clone(),
            Block::List { items } => {
                for blocks in items {
                    collect(blocks, page, title, sections);
                }
                continue;
            }
            Block::Quote { blocks } => {
                collect(blocks, page, title, sections);
                continue;
            }
            Block::Table { headers, rows } => {
                for row in std::iter::once(headers).chain(rows) {
                    add(
                        sections,
                        row.iter()
                            .map(|cell| plain_text(cell))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
                continue;
            }
            Block::Rule => continue,
        };
        add(sections, text);
    }
}

fn add(sections: &mut [Section], text: String) {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        sections.last_mut().unwrap().paragraphs.push(text);
    }
}
