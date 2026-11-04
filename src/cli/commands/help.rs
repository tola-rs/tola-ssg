//! `tola help`: documentation pages and the interactive tree browser.
//!
//! Pages are composed in `crate::help` from the packages' own declarations and the bundled
//! translations, so this command loads no caller site: it parses the request, picks the language,
//! and writes the document.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::Result;
use serde::Serialize;
use tola_build::InputScope;

use crate::cancellation::Cancellation;
use crate::cli::HelpArgs;
use crate::cli::args::HelpFormat;
use crate::cli::log::destination;
use crate::cli::output::CommandOutput;
use crate::help::data::{self, Document};
use crate::help::model::{CROSS_REFERENCE_SCHEME, HelpCategory, LinkTarget, PageId};
use crate::help::pages;
use crate::i18n::HelpLanguage;

pub(in crate::cli) fn run(
    args: &HelpArgs,
    language: HelpLanguage,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let target = pages::request(&args.targets)?;
    destination::start(output, &cancellation.token())?;
    if args.interactive && args.targets.is_empty() && crate::cli::interaction::available() {
        return interactive(language, scope, output, cancellation);
    }
    let document = load(&target, language)?;
    write_document(&document, args.format, output, args.interactive)
}

fn load(target: &LinkTarget, language: HelpLanguage) -> Result<Document> {
    let id = target.page().expect("a help request locates a page");
    data::document(&pages::page_of(id, language)?, target)
}

fn write_document(
    document: &Document,
    format: HelpFormat,
    output: &CommandOutput,
    direct: bool,
) -> Result<()> {
    match format {
        HelpFormat::Json => output.write_stdout_line(serde_json::to_string(document)?),
        HelpFormat::Markdown => output.write_stdout(&document.markdown),
        HelpFormat::Text => {
            let rendered = pages::render(
                &document.markdown,
                output.terminal().stdout_columns(),
                output.terminal().stdout_uses_color(),
            );
            if direct {
                output.write_stdout(rendered)
            } else {
                output.write_documentation(rendered)
            }
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum Category {
    Start,
    Configuration,
    Packages,
}

#[derive(Serialize)]
struct Group {
    name: &'static str,
    pages: Vec<String>,
}

#[derive(Serialize)]
struct TreeInput {
    title: &'static str,
    groups: Vec<Group>,
    bodies: BTreeMap<String, String>,
    targets: BTreeMap<String, String>,
}

struct Page {
    title: String,
    target: String,
    category: Category,
    body: String,
}

fn interactive(
    language: HelpLanguage,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let columns = (output.terminal().stdout_columns().unwrap_or(80) * 3 / 5).max(24);
    let pages = collect(language, columns, output.terminal().stdout_uses_color())?;
    let input = tree_input(&pages);
    let selected: Option<String> =
        crate::cli::interaction::run(include_str!("help.nu"), &input, scope, cancellation)?;
    if let Some(address) = selected {
        let next = pages::request(&[address])?;
        let document = load(&next, language)?;
        write_document(&document, HelpFormat::Text, output, false)?;
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PageKey {
    Overview,
    Config(String),
    Package(String),
}

fn page_key(id: &PageId) -> Option<PageKey> {
    match id {
        PageId::Overview => Some(PageKey::Overview),
        PageId::Index(_) => None,
        PageId::Config { section } => Some(PageKey::Config(section.clone())),
        PageId::Package { name } | PageId::PackageSelection { name, .. } => {
            Some(PageKey::Package(name.clone()))
        }
    }
}

fn canonical_target(key: &PageKey) -> LinkTarget {
    match key {
        PageKey::Overview => LinkTarget::Page(PageId::Overview),
        PageKey::Config(section) => LinkTarget::Page(PageId::Config {
            section: section.clone(),
        }),
        PageKey::Package(name) => LinkTarget::Page(PageId::Package { name: name.clone() }),
    }
}

fn page_title(id: &PageId) -> String {
    match id {
        PageId::Overview => "Overview".into(),
        PageId::Config { section } => format!("[{section}]"),
        PageId::Package { name } | PageId::PackageSelection { name, .. } => {
            format!("@tola/{name}")
        }
        PageId::Index(_) => unreachable!("index pages are not leaves"),
    }
}

fn collect(language: HelpLanguage, columns: usize, color: bool) -> Result<Vec<Page>> {
    let mut queue = VecDeque::from([
        LinkTarget::Page(PageId::Overview),
        LinkTarget::Page(PageId::Index(HelpCategory::Config)),
        LinkTarget::Page(PageId::Index(HelpCategory::Packages)),
    ]);
    let mut processed = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut pages = Vec::new();
    while let Some(target) = queue.pop_front() {
        let Some(id) = target.page().cloned() else {
            continue;
        };
        if !processed.insert(id.uri()) {
            continue;
        }
        let document = data::document(&pages::page_of(&id, language)?, &target)?;
        if let Some(key) = page_key(&id) {
            if !seen.insert(key) {
                continue;
            }
            let category = match &id {
                PageId::Overview => Category::Start,
                PageId::Config { .. } => Category::Configuration,
                PageId::Package { .. } | PageId::PackageSelection { .. } => Category::Packages,
                PageId::Index(_) => unreachable!("index pages are not leaves"),
            };
            let body = pages::render(&document.markdown, Some(columns), color);
            pages.push(Page {
                title: page_title(&id),
                target: target.uri(),
                category,
                body,
            });
        }
        for link in &document.links {
            if link.target.starts_with(CROSS_REFERENCE_SCHEME)
                && let Some(next) = LinkTarget::from_uri(&link.target, &id)
                && let Some(next_id) = next.page()
                && let Some(next_key) = page_key(next_id)
                && !seen.contains(&next_key)
            {
                queue.push_back(canonical_target(&next_key));
            }
        }
    }
    Ok(pages)
}

fn tree_input(pages: &[Page]) -> TreeInput {
    let mut groups = vec![
        Group {
            name: "Start",
            pages: Vec::new(),
        },
        Group {
            name: "Configuration",
            pages: Vec::new(),
        },
        Group {
            name: "Packages",
            pages: Vec::new(),
        },
    ];
    let mut bodies = BTreeMap::new();
    let mut targets = BTreeMap::new();
    let mut used = BTreeSet::new();
    for page in pages {
        let mut title = page.title.clone();
        let mut suffix = 2;
        while !used.insert(title.clone()) {
            title = format!("{} ({suffix})", page.title);
            suffix += 1;
        }
        let index = match page.category {
            Category::Start => 0,
            Category::Configuration => 1,
            Category::Packages => 2,
        };
        groups[index].pages.push(title.clone());
        bodies.insert(title.clone(), page.body.clone());
        targets.insert(title, page.target.clone());
    }
    groups.retain(|group| !group.pages.is_empty());
    TreeInput {
        title: "Tola help",
        groups,
        bodies,
        targets,
    }
}
