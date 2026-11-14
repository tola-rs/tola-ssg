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
use crate::help::model::{HelpCategory, LinkTarget, PageId};
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
    ensure!(
        !args.pretty || matches!(args.format, HelpFormat::Json),
        "`--pretty` requires `--format json`"
    );
    let target = pages::request(&args.targets)?;
    destination::start(output, &cancellation.token())?;
    if args.interactive {
        return interactive(language, scope, output, cancellation);
    }
    match args.format {
        HelpFormat::Json => {
            let id = target.page().expect("a help request locates a page");
            let page = pages::page_of(id, language)?;
            let reference = data::reference(page, &target, language)?;
            let json = if args.pretty {
                serde_json::to_string_pretty(&reference)?
            } else {
                serde_json::to_string(&reference)?
            };
            output.write_stdout_line(json)
        }
        HelpFormat::Markdown => output.write_stdout(&load(&target, language)?.markdown),
        HelpFormat::Text => {
            write_text(&load(&target, language)?, &target, language, output).map(|_| ())
        }
    }
}

fn load(target: &LinkTarget, language: HelpLanguage) -> Result<Document> {
    let id = target.page().expect("a help request locates a page");
    data::document(&pages::page_of(id, language)?, target)
}

fn write_text(
    document: &Document,
    target: &LinkTarget,
    language: HelpLanguage,
    output: &CommandOutput,
) -> Result<bool> {
    let markdown = terminal_markdown(document, target, language);
    let rendered = pages::render(
        &markdown,
        output.terminal().stdout_columns(),
        output.terminal().stdout_uses_color(),
    );
    output.write_documentation(rendered)
}

fn terminal_markdown(document: &Document, target: &LinkTarget, language: HelpLanguage) -> String {
    let mut markdown = document.markdown.clone();
    if let LinkTarget::Page(PageId::Package { name }) = target
        && let Some(package) =
            tola_packages::builtin_packages().find(|package| package.spec().name == name.as_str())
    {
        let examples = package.exports().take(2).collect::<Vec<_>>().join(" ");
        if !examples.is_empty() {
            let (label, lang) = match language {
                HelpLanguage::English => ("Read selected exports", ""),
                HelpLanguage::SimplifiedChinese => ("只查看指定成员", " --lang zh"),
            };
            markdown.push_str(&format!(
                "\n{label}{}`tola help{lang} \"@tola/{name}\" {examples}`\n",
                language.colon()
            ));
        }
    }
    markdown
}

#[derive(Serialize)]
struct Group {
    name: &'static str,
    pages: Vec<String>,
}

#[derive(Serialize)]
struct TreeInput {
    title: &'static str,
    language: &'static str,
    overview: &'static str,
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
    let mut input = tree_input(
        pages,
        contents_columns,
        output.terminal().has_pager(),
        language,
    );
    while let Some(selected) = crate::cli::interaction::run::<Selection>(
        include_str!("help.nu"),
        &input,
        scope,
        cancellation,
    )? {
        let next = pages::request(&[selected.target])?;
        let document = load(&next, language)?;
        if !write_text(&document, &next, language, output)? {
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

fn overview_name(language: HelpLanguage) -> &'static str {
    match language {
        HelpLanguage::English => "Overview",
        HelpLanguage::SimplifiedChinese => "概览",
    }
}

fn page_title(id: &PageId, language: HelpLanguage) -> String {
    match id {
        PageId::Overview => overview_name(language).into(),
        PageId::Config { section } => format!("[{section}]"),
        PageId::Package { name } | PageId::PackageSelection { name, .. } => {
            format!("@tola/{name}")
        }
        PageId::Script { name } => format!("@tola-scripts/{name}"),
        PageId::Index(category) => group_name(*category, language).into(),
    }
}

fn group_name(category: HelpCategory, language: HelpLanguage) -> &'static str {
    match (category, language) {
        (HelpCategory::Config, HelpLanguage::English) => "Configuration",
        (HelpCategory::Packages, HelpLanguage::English) => "Packages",
        (HelpCategory::Scripts, HelpLanguage::English) => "Scripts",
        (HelpCategory::Config, HelpLanguage::SimplifiedChinese) => "配置",
        (HelpCategory::Packages, HelpLanguage::SimplifiedChinese) => "内置包",
        (HelpCategory::Scripts, HelpLanguage::SimplifiedChinese) => "脚本",
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
        let body = pages::render(
            &terminal_markdown(&document, &target, language),
            Some(columns),
            color,
        );
        let title = page_title(&id, language);
        let sections = search::sections(&page, &title);
        pages.push(Page {
            title,
            target: target.uri(),
            group,
            body,
            sections,
        });
        for link in &document.links {
            if let Some(next_id) = link.page()
                && let Some(next_key) = page_key(next_id)
                && !seen.contains(&next_key)
            {
                queue.push_back(canonical_target(&next_key));
            }
        }
    }
    Ok(pages)
}

fn tree_input(
    pages: Vec<Page>,
    contents_columns: usize,
    paged: bool,
    language: HelpLanguage,
) -> TreeInput {
    let categories = [
        HelpCategory::Config,
        HelpCategory::Packages,
        HelpCategory::Scripts,
    ];
    let mut groups = categories.map(|category| Group {
        name: group_name(category, language),
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
        title: match language {
            HelpLanguage::English => "Tola help",
            HelpLanguage::SimplifiedChinese => "Tola 帮助",
        },
        language: language.tag(),
        overview: overview_name(language),
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
