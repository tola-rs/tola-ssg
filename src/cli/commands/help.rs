//! `tola help`: offline reference pages, full reference export, and interactive browser.

use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use tola_input::InputScope;

use crate::cancellation::Cancellation;
use crate::cli::HelpArgs;
use crate::cli::args::HelpFormat;
use crate::cli::log::destination;
use crate::cli::output::CommandOutput;
use crate::help::language::HelpLanguage;
use crate::help::model::{HelpCategory, HelpPage, PageId};
use crate::help::{pages, render, search};

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

    if args.all {
        destination::start(output, &cancellation.token())?;
        let reference = pages::reference(language)?;
        return match args.format {
            HelpFormat::Json => write_json(&reference, args.pretty, output),
            HelpFormat::Markdown => {
                output.write_stdout(render::markdown_reference(&reference, language))
            }
            HelpFormat::Text => {
                let rendered = render::terminal_reference(
                    &reference,
                    language,
                    output.terminal().stdout_columns(),
                    output.terminal().stdout_uses_color(),
                );
                output.write_documentation(rendered).map(|_| ())
            }
        };
    }

    let target = pages::request(&args.targets)?;
    destination::start(output, &cancellation.token())?;
    if args.interactive {
        return interactive(language, scope, output, cancellation);
    }

    let page = pages::page_for_target(&target, language)?;
    match args.format {
        HelpFormat::Json => write_json(&page, args.pretty, output),
        HelpFormat::Markdown => output.write_stdout(render::markdown(&page, language)),
        HelpFormat::Text => write_text(&page, language, output).map(|_| ()),
    }
}

fn write_json(value: &impl Serialize, pretty: bool, output: &CommandOutput) -> Result<()> {
    let json = if pretty {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    output.write_stdout_line(json)
}

fn write_text(page: &HelpPage, language: HelpLanguage, output: &CommandOutput) -> Result<bool> {
    let rendered = render::terminal(
        page,
        language,
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

struct InteractivePage {
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
    let collected = collect(
        language,
        preview_columns,
        output.terminal().stdout_uses_color(),
    )?;
    let mut input = tree_input(
        collected,
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
        let page = pages::page_for_target(&next, language)?;
        if !write_text(&page, language, output)? {
            break;
        }
        cancellation.token().ensure_active()?;
        input.last_page = Some(selected.page);
    }
    Ok(())
}

fn overview_name(language: HelpLanguage) -> &'static str {
    match language {
        HelpLanguage::English => "Overview",
        HelpLanguage::SimplifiedChinese => "概览",
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

fn collect(language: HelpLanguage, columns: usize, color: bool) -> Result<Vec<InteractivePage>> {
    let reference = pages::reference(language)?;
    let mut collected = Vec::new();

    let push = |collected: &mut Vec<InteractivePage>,
                id: PageId,
                title: String,
                group: Option<HelpCategory>,
                page: HelpPage| {
        let body = render::terminal(&page, language, Some(columns), color);
        let sections = search::sections(&page, &id, &title);
        collected.push(InteractivePage {
            title,
            target: id.uri(),
            group,
            body,
            sections,
        });
    };

    push(
        &mut collected,
        PageId::Overview,
        overview_name(language).into(),
        None,
        HelpPage::Overview(reference.overview),
    );
    for category in [
        HelpCategory::Config,
        HelpCategory::Packages,
        HelpCategory::Scripts,
    ] {
        let id = PageId::Index(category);
        let page = pages::page_of(&id, language)?;
        push(
            &mut collected,
            id,
            group_name(category, language).into(),
            None,
            page,
        );
    }
    for config in reference.config {
        let id = PageId::Config {
            section: config.section.clone(),
        };
        let title = config.header.clone();
        push(
            &mut collected,
            id,
            title,
            Some(HelpCategory::Config),
            HelpPage::Config(config),
        );
    }
    for package in reference.packages {
        let id = PageId::Package {
            name: package.name.clone(),
        };
        let title = package.title.clone();
        push(
            &mut collected,
            id,
            title,
            Some(HelpCategory::Packages),
            HelpPage::Package(package),
        );
    }
    for script in reference.scripts {
        let id = PageId::Script {
            name: script.name.clone(),
        };
        let title = script.locator.clone();
        push(
            &mut collected,
            id,
            title,
            Some(HelpCategory::Scripts),
            HelpPage::Script(script),
        );
    }
    Ok(collected)
}

fn tree_input(
    pages: Vec<InteractivePage>,
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
