//! Plain text grouped by the help heading that opens it.

use serde::Serialize;

use super::model::{Block, HelpDocument, HelpPage, LinkTarget, PageId, plain_text};

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
            Block::Heading {
                level,
                spans,
                anchor,
                ..
            } => {
                let target = if *level <= 2 {
                    if let Some(export) = page
                        .reference
                        .exports
                        .iter()
                        .find(|export| export.anchor == anchor.as_str())
                        && let PageId::Package { name } = &page.id
                    {
                        PageId::PackageSelection {
                            name: name.clone(),
                            exports: vec![export.name.clone()],
                        }
                        .uri()
                    } else if page
                        .reference
                        .commands
                        .iter()
                        .any(|command| command.anchor == anchor.as_str())
                    {
                        LinkTarget::PageAnchor(page.id.clone(), anchor.clone()).uri()
                    } else {
                        page.id.uri()
                    }
                } else {
                    sections.last().unwrap().target.clone()
                };
                sections.push(Section {
                    page: title.to_owned(),
                    section: plain_text(spans),
                    target,
                    paragraphs: Vec::new(),
                });
                continue;
            }
            Block::Paragraph { spans } => plain_text(spans),
            Block::Code { source, .. } => source.clone(),
            Block::List { items, .. } => {
                for blocks in items {
                    collect(blocks, page, title, sections);
                }
                continue;
            }
            Block::Quote { blocks } => {
                collect(blocks, page, title, sections);
                continue;
            }
            Block::Table { headers, rows, .. } => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::pages;
    use crate::i18n::HelpLanguage;

    #[test]
    fn method_search_opens_its_export_with_import() {
        let id = PageId::Package {
            name: "datalog".into(),
        };
        let page = pages::page_of(&id, HelpLanguage::SimplifiedChinese).unwrap();
        let sections = sections(&page, "@tola/datalog");
        let method = sections
            .iter()
            .find(|section| section.section == "atom")
            .unwrap();
        let target = pages::request(std::slice::from_ref(&method.target)).unwrap();
        let export =
            pages::page_of(target.page().unwrap(), HelpLanguage::SimplifiedChinese).unwrap();
        assert_eq!(export.reference.exports.len(), 1);
        assert_eq!(export.reference.exports[0].name, "relation");
        assert!(export.reference.import.is_some());
    }
}
