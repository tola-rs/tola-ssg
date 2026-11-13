//! `tola help`: documentation pages and the interactive tree browser.
//!
//! Compose configuration, package, and script pages without loading a site.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use tola_input::InputScope;

use crate::cancellation::Cancellation;
use crate::cli::HelpArgs;
use crate::cli::args::HelpFormat;
use crate::cli::log::destination;
use crate::cli::output::CommandOutput;
use crate::help::data::{self, Document};
use crate::help::model::{CROSS_REFERENCE_SCHEME, HelpCategory, LinkTarget, PageId};
use crate::help::{pages, search};
use crate::i18n::HelpLanguage;

pub(in crate::cli) fn run(
    args: &HelpArgs,
    language: HelpLanguage,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    ensure!(
        !args.interactive || crate::cli::interaction::available(),
        "`--interactive` needs a terminal; omit it to print the documentation"
    );
    let target = pages::request(&args.targets)?;
    destination::start(output, &cancellation.token())?;
    if args.interactive {
        return interactive(language, scope, output, cancellation);
    }
    let document = load(&target, language)?;
    write_document(&document, args.format, output)
}

fn load(target: &LinkTarget, language: HelpLanguage) -> Result<Document> {
    let id = target.page().expect("a help request locates a page");
    data::document(&pages::page_of(id, language)?, target)
}

fn write_document(document: &Document, format: HelpFormat, output: &CommandOutput) -> Result<()> {
    match format {
        HelpFormat::Json => output.write_stdout_line(serde_json::to_string(document)?),
        HelpFormat::Markdown => output.write_stdout(&document.markdown),
        HelpFormat::Text => write_text(document, output).map(|_| ()),
    }
}

fn write_text(document: &Document, output: &CommandOutput) -> Result<bool> {
    let rendered = pages::render(
        &document.markdown,
        output.terminal().stdout_columns(),
        output.terminal().stdout_uses_color(),
    );
    output.write_documentation(rendered)
}

#[derive(Serialize)]
struct Group {
    name: &'static str,
    pages: Vec<String>,
}

#[derive(Serialize)]
struct TreeInput {
    title: &'static str,
    contents_columns: usize,
    groups: Vec<Group>,
    bodies: BTreeMap<String, String>,
    targets: BTreeMap<String, String>,
    sections: Vec<search::Section>,
    paged: bool,
    last_page: Option<String>,
}

struct Page {
    title: String,
    target: String,
    group: Option<HelpCategory>,
    body: String,
    sections: Vec<search::Section>,
}

#[derive(Deserialize)]
struct Selection {
    page: String,
    target: String,
}

fn interactive(
    language: HelpLanguage,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let columns = output.terminal().stdout_columns().unwrap_or(80);
    let contents_columns = (columns * 3 / 10).clamp(1, 32);
    // The split handle takes one column and the preview border takes two.
    let preview_columns = columns.saturating_sub(contents_columns + 3).max(1);
    let pages = collect(
        language,
        preview_columns,
        output.terminal().stdout_uses_color(),
    )?;
    let mut input = tree_input(pages, contents_columns, output.terminal().has_pager());
    while let Some(selected) = crate::cli::interaction::run::<Selection>(
        include_str!("help.nu"),
        &input,
        scope,
        cancellation,
    )? {
        let next = pages::request(&[selected.target])?;
        let document = load(&next, language)?;
        if !write_text(&document, output)? {
            break;
        }
        cancellation.token().ensure_active()?;
        input.last_page = Some(selected.page);
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PageKey {
    Overview,
    Config(String),
    Package(String),
    Script(String),
}

fn page_key(id: &PageId) -> Option<PageKey> {
    match id {
        PageId::Overview => Some(PageKey::Overview),
        PageId::Index(_) => None,
        PageId::Config { section } => Some(PageKey::Config(section.clone())),
        PageId::Package { name } | PageId::PackageSelection { name, .. } => {
            Some(PageKey::Package(name.clone()))
        }
        PageId::Script { name } => Some(PageKey::Script(name.clone())),
    }
}

fn canonical_target(key: &PageKey) -> LinkTarget {
    match key {
        PageKey::Overview => LinkTarget::Page(PageId::Overview),
        PageKey::Config(section) => LinkTarget::Page(PageId::Config {
            section: section.clone(),
        }),
        PageKey::Package(name) => LinkTarget::Page(PageId::Package { name: name.clone() }),
        PageKey::Script(name) => LinkTarget::Page(PageId::Script { name: name.clone() }),
    }
}

fn page_title(id: &PageId) -> String {
    match id {
        PageId::Overview => "Overview".into(),
        PageId::Config { section } => format!("[{section}]"),
        PageId::Package { name } | PageId::PackageSelection { name, .. } => {
            format!("@tola/{name}")
        }
        PageId::Script { name } => format!("@tola-scripts/{name}"),
        PageId::Index(category) => group_name(*category).into(),
    }
}

fn group_name(category: HelpCategory) -> &'static str {
    match category {
        HelpCategory::Config => "Configuration",
        HelpCategory::Packages => "Packages",
        HelpCategory::Scripts => "Scripts",
    }
}

fn collect(language: HelpLanguage, columns: usize, color: bool) -> Result<Vec<Page>> {
    let mut queue = VecDeque::from([
        LinkTarget::Page(PageId::Overview),
        LinkTarget::Page(PageId::Index(HelpCategory::Config)),
        LinkTarget::Page(PageId::Index(HelpCategory::Packages)),
        LinkTarget::Page(PageId::Index(HelpCategory::Scripts)),
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
        let page = pages::page_of(&id, language)?;
        let document = data::document(&page, &target)?;
        if let Some(key) = page_key(&id)
            && !seen.insert(key)
        {
            continue;
        }
        let group = match &id {
            PageId::Overview | PageId::Index(_) => None,
            PageId::Config { .. } => Some(HelpCategory::Config),
            PageId::Package { .. } | PageId::PackageSelection { .. } => {
                Some(HelpCategory::Packages)
            }
            PageId::Script { .. } => Some(HelpCategory::Scripts),
        };
        let body = pages::render(&document.markdown, Some(columns), color);
        let title = page_title(&id);
        let sections = search::sections(&page, &title);
        pages.push(Page {
            title,
            target: target.uri(),
            group,
            body,
            sections,
        });
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

fn tree_input(pages: Vec<Page>, contents_columns: usize, paged: bool) -> TreeInput {
    let categories = [
        HelpCategory::Config,
        HelpCategory::Packages,
        HelpCategory::Scripts,
    ];
    let mut groups = categories.map(|category| Group {
        name: group_name(category),
        pages: Vec::new(),
    });
    let mut bodies = BTreeMap::new();
    let mut targets = BTreeMap::new();
    let mut sections = Vec::new();
    for page in pages {
        if let Some(category) = page.group
            && let Some(index) = categories.iter().position(|&item| item == category)
        {
            groups[index].pages.push(page.title.clone());
        }
        bodies.insert(page.title.clone(), page.body);
        targets.insert(page.title, page.target);
        sections.extend(page.sections);
    }
    TreeInput {
        title: "Tola help",
        contents_columns,
        groups: groups
            .into_iter()
            .filter(|group| !group.pages.is_empty())
            .collect(),
        bodies,
        targets,
        sections,
        paged,
        last_page: None,
    }
}
