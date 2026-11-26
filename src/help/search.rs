//! Plain text grouped by section for interactive `tola help -i` search.

use serde::Serialize;

use super::markup::Markup;
use super::model::{Example, HelpPage, LinkTarget, PageId, anchor};

#[derive(Serialize)]
pub(crate) struct Section {
    page: String,
    section: String,
    target: String,
    paragraphs: Vec<String>,
}

pub(crate) fn sections(page: &HelpPage, id: &PageId, title: &str) -> Vec<Section> {
    let mut sections = vec![Section {
        page: title.to_owned(),
        section: String::new(),
        target: id.uri(),
        paragraphs: Vec::new(),
    }];

    match page {
        HelpPage::Overview(overview) => {
            push_markup(&mut sections, &overview.body);
        }
        HelpPage::Index(index) => {
            push_markup(&mut sections, &index.summary);
            for entry in &index.entries {
                push_text(&mut sections, &entry.title);
                push_markup(&mut sections, &entry.summary);
            }
        }
        HelpPage::Config(config) => {
            push_markup(&mut sections, &config.summary);
            push_markup(&mut sections, &config.body);
            push_examples(&mut sections, &config.examples);
            for child in &config.children {
                push_text(&mut sections, &child.header);
                push_markup(&mut sections, &child.summary);
            }
            for key in &config.keys {
                sections.push(Section {
                    page: title.to_owned(),
                    section: key.name.clone(),
                    target: id.uri(),
                    paragraphs: Vec::new(),
                });
                push_markup(&mut sections, &key.body);
                push_examples(&mut sections, &key.examples);
            }
        }
        HelpPage::Package(package) => {
            push_markup(&mut sections, &package.summary);
            push_markup(&mut sections, &package.body);
            push_examples(&mut sections, &package.examples);
            for export in &package.exports {
                let target = PageId::PackageSelection {
                    name: package.name.clone(),
                    exports: vec![export.name.clone()],
                }
                .uri();
                sections.push(Section {
                    page: title.to_owned(),
                    section: format!("{} - {}", export.name, export.kind),
                    target: target.clone(),
                    paragraphs: Vec::new(),
                });
                push_markup(&mut sections, &export.summary);
                push_markup(&mut sections, &export.body);
                for parameter in &export.parameters {
                    push_text(&mut sections, &parameter.name);
                    push_markup(&mut sections, &parameter.description);
                }
                push_examples(&mut sections, &export.examples);
                for method in &export.methods {
                    sections.push(Section {
                        page: title.to_owned(),
                        section: method.name.clone(),
                        target: target.clone(),
                        paragraphs: Vec::new(),
                    });
                    push_markup(&mut sections, &method.summary);
                    push_markup(&mut sections, &method.body);
                    for parameter in &method.parameters {
                        push_text(&mut sections, &parameter.name);
                        push_markup(&mut sections, &parameter.description);
                    }
                    push_examples(&mut sections, &method.examples);
                }
            }
        }
        HelpPage::Script(script) => {
            push_markup(&mut sections, &script.summary);
            push_markup(&mut sections, &script.body);
            for command in &script.commands {
                let target = LinkTarget::PageAnchor(id.clone(), anchor(&command.name)).uri();
                sections.push(Section {
                    page: title.to_owned(),
                    section: command.name.clone(),
                    target,
                    paragraphs: Vec::new(),
                });
                push_markup(&mut sections, &command.summary);
                push_markup(&mut sections, &command.body);
                for parameter in command.parameters.iter().chain(&command.options) {
                    push_text(&mut sections, &parameter.name);
                    push_markup(&mut sections, &parameter.description);
                }
                push_examples(&mut sections, &command.examples);
            }
        }
    }

    sections
}

fn push_markup(sections: &mut [Section], markup: &Markup) {
    let text = markup.plain_text();
    push_text(sections, &text);
}

fn push_examples(sections: &mut [Section], examples: &[Example]) {
    for example in examples {
        if let Some(title) = &example.title {
            push_markup(sections, title);
        }
        if let Some(description) = &example.description {
            push_markup(sections, description);
        }
        push_text(sections, &example.code);
    }
}

fn push_text(sections: &mut [Section], raw: &str) {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        sections.last_mut().unwrap().paragraphs.push(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::language::HelpLanguage;
    use crate::help::pages;

    #[test]
    fn method_search_opens_export() {
        let id = PageId::Package {
            name: "datalog".into(),
        };
        let page = pages::page_of(&id, HelpLanguage::SimplifiedChinese).unwrap();
        let secs = sections(&page, &id, "@tola/datalog");
        let method = secs.iter().find(|s| s.section == "atom").unwrap();
        let target = pages::request(std::slice::from_ref(&method.target)).unwrap();
        let HelpPage::Package(exp) =
            pages::page_of(target.page().unwrap(), HelpLanguage::SimplifiedChinese).unwrap()
        else {
            panic!("expected package page");
        };
        assert_eq!(exp.exports.len(), 1);
        assert_eq!(exp.exports[0].name, "relation");
        assert!(!exp.import.is_empty());
    }
}
