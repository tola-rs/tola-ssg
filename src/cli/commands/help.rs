use anyhow::Result;

use tola_build::config::section::{
    AssetsConfig, FontsConfig, IconsConfig, SiteSectionConfig, TypstSectionConfig, VendorConfig,
    build::{
        BuildSectionConfig, HooksConfig, MinifyConfig, ReferencesConfig,
        hooks::{AfterPublishHookConfig, BeforeBuildHookConfig, OutputCommandConfig},
    },
};
use tola_build::diagnostic::{Diagnostic, DiagnosticCode, DiagnosticError, Severity};
use tola_packages::{ExportDeclaration, RelatedTarget, Signature};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::config::{DevConfig, DiagnosticsConfig, ServerConfig};
use crate::help::model::{Anchor, HelpDocument, HelpPage, LinkTarget, PageId, anchor};
use crate::help::view::View;
use crate::i18n::{HelpLanguage, HelpText, PackageTranslations};
use crate::terminal::documentation::Documentation;
use crate::terminal::session::{self, Shown};

type Template = fn() -> std::result::Result<String, tola_config::ConfigTemplateError>;

struct ConfigTable {
    section: &'static str,
    array: bool,
    template: Template,
}

impl ConfigTable {
    fn header(&self) -> String {
        if self.array {
            format!("[[{}]]", self.section)
        } else {
            format!("[{}]", self.section)
        }
    }
}

const TABLES: &[ConfigTable] = &[
    ConfigTable {
        section: SiteSectionConfig::TEMPLATE_SECTION,
        array: false,
        template: SiteSectionConfig::try_template_with_header,
    },
    ConfigTable {
        section: BuildSectionConfig::TEMPLATE_SECTION,
        array: false,
        template: BuildSectionConfig::try_template_with_header,
    },
    ConfigTable {
        section: MinifyConfig::TEMPLATE_SECTION,
        array: false,
        template: MinifyConfig::try_template_with_header,
    },
    ConfigTable {
        section: ReferencesConfig::TEMPLATE_SECTION,
        array: false,
        template: ReferencesConfig::try_template_with_header,
    },
    ConfigTable {
        section: HooksConfig::TEMPLATE_SECTION,
        array: false,
        template: HooksConfig::try_template_with_header,
    },
    ConfigTable {
        section: BeforeBuildHookConfig::TEMPLATE_SECTION,
        array: true,
        template: BeforeBuildHookConfig::try_template_with_header,
    },
    ConfigTable {
        section: OutputCommandConfig::TEMPLATE_SECTION,
        array: true,
        template: OutputCommandConfig::try_template_with_header,
    },
    ConfigTable {
        section: AfterPublishHookConfig::TEMPLATE_SECTION,
        array: true,
        template: AfterPublishHookConfig::try_template_with_header,
    },
    ConfigTable {
        section: AssetsConfig::TEMPLATE_SECTION,
        array: false,
        template: AssetsConfig::try_template_with_header,
    },
    ConfigTable {
        section: TypstSectionConfig::TEMPLATE_SECTION,
        array: false,
        template: TypstSectionConfig::try_template_with_header,
    },
    ConfigTable {
        section: FontsConfig::TEMPLATE_SECTION,
        array: false,
        template: FontsConfig::try_template_with_header,
    },
    ConfigTable {
        section: IconsConfig::TEMPLATE_SECTION,
        array: false,
        template: IconsConfig::try_template_with_header,
    },
    ConfigTable {
        section: VendorConfig::TEMPLATE_SECTION,
        array: false,
        template: VendorConfig::try_template_with_header,
    },
    ConfigTable {
        section: ServerConfig::TEMPLATE_SECTION,
        array: false,
        template: ServerConfig::try_template_with_header,
    },
    ConfigTable {
        section: DevConfig::TEMPLATE_SECTION,
        array: false,
        template: DevConfig::try_template_with_header,
    },
    ConfigTable {
        section: DiagnosticsConfig::TEMPLATE_SECTION,
        array: false,
        template: DiagnosticsConfig::try_template_with_header,
    },
];

pub(in crate::cli) fn run(
    targets: &[String],
    language: HelpLanguage,
    interactive: bool,
    mouse: bool,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    if interactive {
        return run_interactive(targets, language, mouse, output, cancellation);
    }
    output.write_documentation(render(
        targets,
        language,
        output.terminal().stdout_columns(),
        output.terminal().stdout_uses_color(),
    )?)?;
    Ok(())
}

/// Shows the requested page as an interactive view, or prints it when no terminal can draw it.
fn run_interactive(
    targets: &[String],
    language: HelpLanguage,
    mouse: bool,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let requested = page(targets, language, CrossRefs::Emit)?;
    let load = |id: &PageId| page_of(id, language, CrossRefs::Emit).map(HelpDocument::parse);
    let mut view = View::new(HelpDocument::parse(requested), &load);
    view.set_mouse(mouse);
    let token = cancellation.token();
    let sink = output.terminal().sink();
    let cancelled = || token.is_cancelled();
    match session::show(&sink, output.terminal().palette(), &cancelled, &mut view) {
        Ok(Shown::Interactive) => Ok(()),
        // A terminal that cannot draw a frame gets the page the plain path writes, pager and all.
        Ok(Shown::Plain) => {
            let text = render(
                targets,
                language,
                output.terminal().stdout_columns(),
                output.terminal().stdout_uses_color(),
            )?;
            output.write_documentation(text)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Whether a page writes its cross-references as links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrossRefs {
    /// Production: a code span that names another page holds its `tola://` reference.
    Emit,
    /// The same page without links, so a test can prove the links add no visible text.
    #[allow(dead_code)] // Production emits links; the tests compare both modes.
    Suppress,
}

/// The page one request shows: the Markdown the plain renderer and the interactive view share.
fn page(targets: &[String], language: HelpLanguage, cross_refs: CrossRefs) -> Result<HelpPage> {
    match targets.first().map(String::as_str) {
        None => Ok(overview_page(language, cross_refs)),
        Some(target) if target.starts_with('[') => {
            table_page(target, targets, language, cross_refs)
        }
        Some(target) if target.starts_with('@') => {
            package_page(target, targets, language, cross_refs)
        }
        Some(target) => Err(target_error(
            target,
            "Use `tola <command> --help` for command options",
        )),
    }
}

/// The page one id names, built as the request that names it builds it.
fn page_of(id: &PageId, language: HelpLanguage, cross_refs: CrossRefs) -> Result<HelpPage> {
    match id {
        PageId::Overview => Ok(overview_page(language, cross_refs)),
        PageId::Table { section, array } => {
            let target = if *array {
                format!("[[{section}]]")
            } else {
                format!("[{section}]")
            };
            let targets = vec![target];
            table_page(&targets[0], &targets, language, cross_refs)
        }
        PageId::Package { name } => {
            let targets = vec![name.clone()];
            package_page(&targets[0], &targets, language, cross_refs)
        }
        PageId::PackageSelection { name, exports } => {
            let mut targets = Vec::with_capacity(exports.len() + 1);
            targets.push(name.clone());
            targets.extend(exports.iter().cloned());
            package_page(&targets[0], &targets, language, cross_refs)
        }
    }
}

/// One named token, linked to the page it names when the page emits cross-references.
///
/// Only the token is linked: a link adds no styling of its own, so the page renders the same
/// bytes with and without its links, and the plain renderer hides the reserved target. The
/// command around the token stays literal text, so the quoting the reader copies stays intact.
fn cross_reference(
    token: &str,
    target: &LinkTarget,
    page: &PageId,
    cross_refs: CrossRefs,
) -> String {
    match cross_refs {
        CrossRefs::Emit => format!("[`{token}`]({})", target.uri(page)),
        CrossRefs::Suppress => format!("`{token}`"),
    }
}

/// One named token, linked to the page it names when the index can resolve it; a mention the
/// index cannot resolve stays plain text, because the index outlives a rename.
fn named(token: &str, target: Option<LinkTarget>, page: &PageId, cross_refs: CrossRefs) -> String {
    match target {
        Some(target) => cross_reference(token, &target, page, cross_refs),
        None => format!("`{token}`"),
    }
}

/// What an export's own section calls it.
fn export_kind(export: &tola_packages::ExportDocumentation) -> &'static str {
    if export.declaration.is_function() {
        "function"
    } else {
        "value"
    }
}

/// The anchor of an export's own section, on the page that renders it.
fn export_anchor(export: &tola_packages::ExportDocumentation) -> Anchor {
    anchor(&format!("{} {} {}", export.name, '-', export_kind(export)))
}

fn render(
    targets: &[String],
    language: HelpLanguage,
    columns: Option<usize>,
    color: bool,
) -> Result<String> {
    let documentation = Documentation::new(columns, color);
    let page = page(targets, language, CrossRefs::Emit)?;
    let mut text = documentation.render(&page.markdown());
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// The index page: worked examples, every whole configuration table, and every bundled package.
///
/// The index lists each section once: a table another table names is reached through that
/// table's page, so it stays out. A named token has the link, so the letters jump mode draws
/// land on the name itself: the command around it stays literal text, and the quoting the reader
/// copies stays intact.
fn overview_page(language: HelpLanguage, cross_refs: CrossRefs) -> HelpPage {
    let listed = |token: &str, target: Option<LinkTarget>| {
        format!("- {}", named(token, target, &PageId::Overview, cross_refs))
    };
    // The index lists every table: a listed name is the selector itself, without the shell
    // quoting the command line needs. A container with no key of its own and one child table
    // has nothing else, so it is listed as that child instead.
    let tables = TABLES
        .iter()
        .filter(|table| !is_empty_container(table.section))
        .map(|table| sole_child_table(table.section).unwrap_or(table))
        .map(|table| listed(&table.header(), table_target(table.section)))
        .collect::<Vec<_>>()
        .join("\n");
    let packages = tola_packages::builtin_packages()
        .map(|package| {
            let name = package.spec().name.to_string();
            listed(&format!("@tola/{name}"), package_target(&name, &[]))
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Only the token is linked, so the command around it stays literal and the quoting the
    // reader copies stays intact. A package is named by its bare name here; the token the reader
    // copies includes the `@tola/` scope.
    let command = |name: &str, exports: &[&str]| {
        let target = format!("@tola/{name}");
        let selection = package_target(name, exports);
        let named_exports = exports
            .iter()
            .map(|export| named(export, selection.clone(), &PageId::Overview, cross_refs))
            .collect::<Vec<_>>()
            .join(" ");
        let target = named(&target, selection, &PageId::Overview, cross_refs);
        match named_exports.is_empty() {
            true => format!("`tola help` \"{target}\""),
            false => format!("`tola help` \"{target}\" {named_exports}"),
        }
    };
    let table = |header: &str, section: &str| {
        let table = named(header, table_target(section), &PageId::Overview, cross_refs);
        format!("`tola help` \"{table}\"")
    };
    let text = |phrase: HelpText| crate::i18n::text(language, phrase);
    let examples = [
        format!(
            "{} - {}",
            table("[site]", SiteSectionConfig::TEMPLATE_SECTION),
            text(HelpText::SiteFields)
        ),
        format!(
            "{} - {}",
            command("web", &[]),
            text(HelpText::PackageOverview)
        ),
        format!(
            "{} - {}",
            command("address", &["slugify", "output-to-url"]),
            text(HelpText::SelectedExports)
        ),
        table("[build.hooks]", "build.hooks"),
        command("document", &["headings", "references"]),
        command("code", &["code-themes"]),
    ]
    .join("\n- ");
    let markdown = format!(
        "## Tola help pages:\n\n\
         - {examples}\n\n\
         ## Configuration tables:\n\n{tables}\n\n## Bundled packages:\n\n{packages}\n"
    );
    HelpPage::new(PageId::Overview, markdown)
}

/// The table page one section names, when the index names a real table.
fn table_target(section: &str) -> Option<LinkTarget> {
    TABLES
        .iter()
        .find(|table| table.section == section)
        .map(|table| {
            LinkTarget::Page(PageId::Table {
                section: table.section.to_owned(),
                array: table.array,
            })
        })
}

/// The package page one bundled package names, when every named export belongs to it.
fn package_target(name: &str, exports: &[&str]) -> Option<LinkTarget> {
    let package = tola_packages::builtin_packages().find(|package| package.spec().name == name)?;
    if !exports
        .iter()
        .all(|export| package.exports().any(|name| name == *export))
    {
        return None;
    }
    let name = format!("@tola/{name}");
    Some(LinkTarget::Page(match exports {
        [] => PageId::Package { name },
        exports => PageId::PackageSelection {
            name,
            exports: exports.iter().map(|export| (*export).to_owned()).collect(),
        },
    }))
}

fn table_page(
    target: &str,
    targets: &[String],
    language: HelpLanguage,
    cross_refs: CrossRefs,
) -> Result<HelpPage> {
    require_single(targets)?;
    let table = TABLES
        .iter()
        .find(|table| table.header() == target)
        .ok_or_else(|| target_error(target, &table_choices()))?;
    // A container whose only content is its one child shows that child's page instead, so
    // the reader lands on the table that names real settings.
    if let Some(child) = sole_child_table(table.section) {
        return table_page(&child.header(), &[child.header()], language, cross_refs);
    }
    let id = PageId::Table {
        section: table.section.to_owned(),
        array: table.array,
    };
    let template = (table.template)()?;
    let mut markdown = format!("# {} - configuration table\n", table.header());
    let translations = crate::i18n::sections(language);
    if let Some(documentation) = documented(table.section).map(|english| {
        translations
            .and_then(|tables| tables.documentation(table.section))
            .unwrap_or(english)
    }) {
        markdown.push_str(&format!("\n{documentation}\n"));
    }
    if template.trim().is_empty() {
        markdown.push_str("\nConfigure this section through its child tables:\n");
        for child in child_tables(table.section) {
            let child_id = PageId::Table {
                section: child.section.to_owned(),
                array: child.array,
            };
            let header = child.header();
            // A table header is an array when its declaration is, so it includes its own brackets.
            let table = cross_reference(&header, &LinkTarget::Page(child_id), &id, cross_refs);
            markdown.push_str(&format!("- `tola help` \"{table}\"\n"));
        }
    } else {
        markdown.push_str(&format!(
            "\n{}\n\n",
            crate::i18n::text(language, HelpText::DefaultValues)
        ));
        markdown.push_str(&code_block("toml", &template));
        markdown.push_str(&declared_keys(table.section, &id, translations, cross_refs));
    }
    if let Some(english) = section_help(table.section) {
        let help = translations
            .and_then(|tables| tables.help(table.section))
            .unwrap_or(english);
        markdown.push_str(&format!("\n{help}\n"));
    }
    if table.section == SiteSectionConfig::TEMPLATE_SECTION {
        // The section template hides the author-chosen map, so its parent help names it and
        // holds the declaration documentation a per-key query cannot reach.
        let english = documented("site.extra").ok_or_else(|| {
            anyhow::anyhow!("configuration `site.extra` has no declaration documentation")
        })?;
        let docs = translations
            .and_then(|tables| tables.field("site.extra"))
            .unwrap_or(english);
        let heading = cross_reference(
            "[site.extra]",
            &LinkTarget::PageAnchor(id.clone(), anchor("Map-shaped [site.extra]")),
            &id,
            cross_refs,
        );
        markdown.push_str(&format!("\n## Map-shaped {heading}\n\n{docs}\n"));
    }
    Ok(HelpPage::new(id, markdown))
}

/// Every key this section's own page lists, in schema order: the keys its declaration writes,
/// without the nested ones. A key that opens a child table points at that table's page, and the
/// map-shaped `[site.extra]` is explained by the table that holds it.
///
/// The renderer and the translation guard share this one enumeration.
fn documented_fields(section: &str) -> Vec<&'static str> {
    let prefix = format!("{section}.");
    tola_build::config::field_paths()
        .map(|path| path.as_str())
        .chain(crate::config::field_paths().map(|path| path.as_str()))
        .filter_map(|path| {
            let name = path.strip_prefix(&prefix)?;
            if name.contains('.') || path == "site.extra" {
                return None;
            }
            documented(path).map(|_| path)
        })
        .collect()
}

/// The schema's own documentation of one declared key, when it has any.
///
/// A core key is answered by `tola-build` and a `[server]`, `[dev]`, or `[diagnostics]` key by
/// this host, so the two declarations read as one schema.
fn documented(path: &str) -> Option<&'static str> {
    tola_build::config::field_documentation(path)
        .or_else(|| crate::config::field_documentation(path))
}

/// The prose a table's page ends with: the core schema's own help, or the host's.
fn section_help(section: &str) -> Option<&'static str> {
    tola_build::config::section_help(section).or_else(|| crate::config::section_help(section))
}

/// The child table one key opens, when its own page documents it.
fn child_table(path: &str) -> Option<&'static ConfigTable> {
    TABLES.iter().find(|child| child.section == path)
}

/// Every table whose own page is reached through `section`'s page, in schema order.
fn child_tables(section: &str) -> impl Iterator<Item = &'static ConfigTable> {
    let prefix = format!("{section}.");
    TABLES.iter().filter(move |table| {
        table
            .section
            .strip_prefix(&prefix)
            .is_some_and(|suffix| !suffix.contains('.'))
    })
}

/// Whether a table holds nothing but one child table: no key of its own, and exactly one key
/// opening a table, so that table's page is the whole content the container has.
fn is_empty_container(section: &str) -> bool {
    let mut children = child_tables(section);
    let Some(_) = children.next() else {
        return false;
    };
    if children.next().is_some() {
        return false;
    }
    documented_fields(section)
        .iter()
        .all(|path| child_table(path).is_some())
}

/// The one child table `section` reaches, when a container has nothing but it.
///
/// A table that declares no key of its own still reaches its child through the key naming it, so
/// the child is the whole content: the index lists the child, and the container's page shows it.
fn sole_child_table(section: &str) -> Option<&'static ConfigTable> {
    is_empty_container(section)
        .then(|| child_tables(section).next())
        .flatten()
}

/// Every key the section's own declaration writes, with what each one means.
///
/// The template prints the values; the meaning is the declaration's documentation, and a key that
/// opens a child table points at the table that documents it. One key's documentation is one
/// rendered line: the page wraps it.
fn declared_keys(
    section: &str,
    id: &PageId,
    translations: Option<&crate::i18n::SectionTranslations>,
    cross_refs: CrossRefs,
) -> String {
    let mut markdown = String::new();
    for path in documented_fields(section) {
        let name = path
            .strip_prefix(section)
            .unwrap_or(path)
            .trim_start_matches('.');
        let meaning = match child_table(path) {
            Some(child) => cross_reference(
                &child.header(),
                &LinkTarget::Page(PageId::Table {
                    section: child.section.to_owned(),
                    array: child.array,
                }),
                id,
                cross_refs,
            ),
            None => {
                let english = documented(path).expect("the enumeration documents every field");
                one_line(
                    translations
                        .and_then(|tables| tables.field(path))
                        .unwrap_or(english),
                )
            }
        };
        markdown.push_str(&format!("\n- `{name}` - {meaning}"));
    }
    markdown.push('\n');
    markdown
}

/// One documentation paragraph whose lines the rendered page wraps itself.
fn one_line(documentation: &str) -> String {
    documentation
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn package_page(
    target: &str,
    targets: &[String],
    language: HelpLanguage,
    cross_refs: CrossRefs,
) -> Result<HelpPage> {
    let package = tola_packages::builtin_packages()
        .find(|package| format!("@tola/{}", package.spec().name) == target)
        .ok_or_else(|| target_error(target, &package_choices()))?;
    for member in &targets[1..] {
        if !package.exports().any(|name| name == member) {
            return Err(help_error(
                crate::codes::help::MEMBER,
                format!("`{member}` is not an export of `{target}`"),
                format!(
                    "Exports: {}",
                    package.exports().map(quoted).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
    }
    let whole_package = targets.len() == 1;
    let names = if whole_package {
        package.exports().map(str::to_owned).collect::<Vec<_>>()
    } else {
        targets[1..].to_vec()
    };
    let imports = if whole_package {
        "*".to_owned()
    } else {
        names.join(", ")
    };
    let id = if whole_package {
        PageId::Package {
            name: target.to_owned(),
        }
    } else {
        PageId::PackageSelection {
            name: target.to_owned(),
            exports: names.clone(),
        }
    };
    let translations = crate::i18n::package(language, &package.spec().name);
    let import = format!("#import \"{}\": {imports}", package.spec());
    let mut text = format!(
        "# {target}\n\n## Import in Typst:\n\n{}",
        code_block("typst", &import)
    );
    if whole_package && let Some(overview) = package.overview() {
        let overview = translations
            .and_then(|translations| translations.overview())
            .unwrap_or(&overview);
        text.push_str(&format!("\n{overview}\n"));
    }
    let exports = package.export_documentation(&names)?;
    if whole_package {
        text.push_str("\n## Exports:\n\n");
        for export in &exports {
            let localized = translations
                .and_then(|translations| translations.summary_prose(&export.name))
                .map(|prose| crate::i18n::localize_summary(&export.documentation.summary, prose));
            let summary = localized
                .as_deref()
                .map(summary_lead)
                .or_else(|| export_summary(export));
            let name = named(
                &export.name,
                Some(LinkTarget::PageAnchor(id.clone(), export_anchor(export))),
                &id,
                cross_refs,
            );
            match summary {
                Some(summary) => text.push_str(&format!("- {name} - {summary}\n")),
                None => text.push_str(&format!("- {name}\n")),
            }
        }
        let example = names
            .iter()
            .take(2)
            .map(|name| format!(" {name}"))
            .collect::<String>();
        text.push_str(&format!(
            "\nRead selected exports: `tola help \"{target}\"{example}`\n"
        ));
    }
    let mut page = HelpPage::new(id.clone(), text);
    for (position, export) in exports.iter().enumerate() {
        let mut text = String::new();
        if position > 0 {
            // One rule between whole API sections: the reader sees where one ends.
            text.push_str("\n---\n");
        }
        text.push_str(&format!(
            "\n## {} - {}\n\n{}\n",
            export.name,
            export_kind(export),
            code_block("typst-code", &declaration(&export.declaration))
        ));
        if let ExportDeclaration::Function(signature) = &export.declaration {
            text.push_str(&parameter_sections(translations, &export.name, signature));
        }
        let documentation = &export.documentation;
        let localized = translations
            .and_then(|translations| translations.summary_prose(&export.name))
            .map(|prose| crate::i18n::localize_summary(&export.documentation.summary, prose));
        let summary = localized.as_deref().unwrap_or(&documentation.summary);
        if !summary.is_empty() {
            text.push_str(&format!("\n{summary}\n"));
        }
        text.push_str(&related_section(
            target,
            &export.related,
            &names,
            &id,
            cross_refs,
        ));
        page.push_export(text);
    }
    Ok(page)
}

/// The declaration block every export is configured with: a function's signature, or a value's
/// own line.
fn declaration(declaration: &ExportDeclaration) -> String {
    match declaration {
        ExportDeclaration::Function(signature) => signature.code(),
        ExportDeclaration::Value(value) => value.clone(),
    }
}

/// The parameter sections, in the order the signature spells the parameters: each group headed,
/// each parameter named with the type it renders, then what the documentation says about it. The
/// signature already shows which parameters are optional and which collect arguments.
fn parameter_sections(
    translations: Option<&PackageTranslations>,
    export: &str,
    signature: &Signature,
) -> String {
    let mut text = String::new();
    for (heading, group) in [
        ("Positional Parameters", signature.positional()),
        ("Rest Parameters", signature.rest()),
        ("Named Parameters", signature.named()),
    ] {
        if group.is_empty() {
            continue;
        }
        text.push_str(&format!("\n### {heading}\n"));
        for parameter in group {
            text.push_str(&format!("\n#### {}\n", parameter.name));
            text.push_str(&format!(
                "\n{}\n",
                code_block("typst-code", &format!("type: {}", parameter.ty_spelling()))
            ));
            let documentation = translations
                .and_then(|translations| translations.parameter(export, &parameter.name))
                .or(parameter.docs.as_deref());
            if let Some(documentation) = documentation {
                text.push_str(&format!("\n{documentation}\n"));
            }
        }
    }
    text
}

/// The related-targets section one export renders: the package's own exports as one command,
/// then each bundled package as its own, in the order the block first names them. A target the
/// page renders anyway is left out, and a block whose targets are all rendered has no section.
fn related_section(
    target: &str,
    related: &[RelatedTarget],
    rendered: &[String],
    page: &PageId,
    cross_refs: CrossRefs,
) -> String {
    let mut exports: Vec<&str> = Vec::new();
    let mut packages: Vec<&str> = Vec::new();
    for related in related {
        let (name, names) = match related {
            RelatedTarget::Export(name) => (name.as_str(), &mut exports),
            RelatedTarget::Package(name) => {
                if target.strip_prefix("@tola/") == Some(name.as_str()) {
                    continue;
                }
                (name.as_str(), &mut packages)
            }
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    exports.retain(|name| !rendered.iter().any(|export| export == name));
    if exports.is_empty() && packages.is_empty() {
        return String::new();
    }
    let mut text = String::from("\n### Related\n\n");
    if !exports.is_empty() {
        let listed = exports
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let selection = PageId::PackageSelection {
            name: target.to_owned(),
            exports: exports.iter().map(|name| (*name).to_owned()).collect(),
        };
        let named_exports = exports
            .iter()
            .map(|name| {
                cross_reference(name, &LinkTarget::Page(selection.clone()), page, cross_refs)
            })
            .collect::<Vec<_>>()
            .join(" ");
        text.push_str(&format!(
            "- {listed} - `tola help` \"{target}\" {named_exports}\n"
        ));
    }
    for package in packages {
        let name = cross_reference(
            &format!("@tola/{package}"),
            &LinkTarget::Page(PageId::Package {
                name: format!("@tola/{package}"),
            }),
            page,
            cross_refs,
        );
        text.push_str(&format!("- {name} - `tola help \"@tola/{package}\"`\n"));
    }
    text
}

fn code_block(language: &str, source: &str) -> String {
    let longest = source
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let newline = if source.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{language}\n{source}{newline}{fence}\n")
}

fn require_single(targets: &[String]) -> Result<()> {
    if targets.len() != 1 {
        return Err(help_error(
            crate::codes::help::TARGET,
            format!("`{}` takes no member selectors", targets[0]),
            "Use `tola help \"@tola/package\" export1 export2` for selected exports".into(),
        ));
    }
    Ok(())
}

fn table_choices() -> String {
    format!(
        "Configuration tables: {}",
        table_selectors().collect::<Vec<_>>().join(", ")
    )
}

fn package_choices() -> String {
    format!(
        "Bundled packages: {}",
        package_selectors().collect::<Vec<_>>().join(", ")
    )
}

/// The first non-empty line of one documented unit.
fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// The line one translated summary shows in an export index.
///
/// English indexes by a summary's first wrapped line, which the package sources break at a
/// sentence end. A translation keeps each paragraph on one line, so it indexes by the first
/// sentence instead.
fn summary_lead(summary: &str) -> &str {
    let summary = summary.trim();
    match summary
        .char_indices()
        .find(|(_, character)| matches!(character, '。' | '！' | '？'))
    {
        Some((end, character)) => &summary[..end + character.len_utf8()],
        None => summary,
    }
}

/// The first documented line of an export, as its line in a package's export index.
fn export_summary(export: &tola_packages::ExportDocumentation) -> Option<&str> {
    first_line(&export.documentation.summary)
}

// Quotes keep table headers from being expanded as shell patterns.
fn quoted(selector: &str) -> String {
    format!("\"{selector}\"")
}

/// Every configuration-table selector, shared by the no-argument index and the error help.
fn table_selectors() -> impl Iterator<Item = String> {
    TABLES.iter().map(|table| quoted(&table.header()))
}

/// Every bundled-package selector, shared by the no-argument index and the error help.
fn package_selectors() -> impl Iterator<Item = String> {
    tola_packages::builtin_packages()
        .map(|package| quoted(&format!("@tola/{}", package.spec().name)))
}

fn target_error(target: &str, choices: &str) -> anyhow::Error {
    help_error(
        crate::codes::help::TARGET,
        format!("`{target}` is not a help target"),
        choices.to_owned(),
    )
}

fn help_error(code: DiagnosticCode, message: String, choices: String) -> anyhow::Error {
    let diagnostic = Diagnostic::new(code, Severity::Error, message.clone()).with_help(choices);
    DiagnosticError::new(message, vec![diagnostic]).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::model::{Block, CROSS_REFERENCE_SCHEME, Inline};

    fn request(targets: &[&str]) -> Result<String> {
        request_in(HelpLanguage::English, targets)
    }

    fn request_in(language: HelpLanguage, targets: &[&str]) -> Result<String> {
        render(
            &targets
                .iter()
                .map(|target| (*target).to_owned())
                .collect::<Vec<_>>(),
            language,
            None,
            false,
        )
    }

    /// Every request the command can show: the index, every table, every package, and a
    /// two-export selection of each package.
    fn requests() -> Vec<Vec<String>> {
        let mut requests = vec![Vec::new()];
        requests.extend(TABLES.iter().map(|table| vec![table.header()]));
        for package in tola_packages::builtin_packages() {
            let target = format!("@tola/{}", package.spec().name);
            requests.push(vec![target.clone()]);
            let mut selection = vec![target];
            selection.extend(package.exports().take(2).map(str::to_owned));
            requests.push(selection);
        }
        requests
    }

    /// Every documented unit has a translation, and no catalog key outlives its unit: a
    /// section's own documentation, the help its page ends with, and every field's meaning.
    ///
    /// A key that opens a child table is documented by that table's page, so it has no
    /// translation of its own; the map-shaped `[site.extra]` is explained by its `[site]` page.
    #[test]
    fn section_translations_stay_in_step() {
        let tables = crate::i18n::sections(HelpLanguage::SimplifiedChinese)
            .expect("the bundled tables include Simplified Chinese");
        for table in TABLES {
            let section = table.section;
            assert_eq!(
                documented(section).is_some(),
                tables.documentation(section).is_some(),
                "`{section}` documentation and its translation must both exist"
            );
            assert_eq!(
                section_help(section).is_some(),
                tables.help(section).is_some(),
                "`{section}` help and its translation must both exist"
            );
            for path in documented_fields(section) {
                if child_table(path).is_some() {
                    continue;
                }
                assert!(tables.field(path).is_some(), "`{path}` has no translation");
            }
            if section == SiteSectionConfig::TEMPLATE_SECTION {
                assert!(
                    tables.field("site.extra").is_some(),
                    "`site.extra` has no translation"
                );
            }
        }
        for name in tables.section_names() {
            assert!(
                TABLES.iter().any(|table| table.section == name),
                "`{name}` is not a configuration table"
            );
        }
        for path in tables.field_paths() {
            let shown = path == "site.extra"
                || TABLES
                    .iter()
                    .any(|table| documented_fields(table.section).contains(&path));
            assert!(shown, "`{path}` is not a key a table page documents");
        }
    }

    /// The Markdown one request builds, before any renderer reads it.
    fn markup(targets: &[String], cross_refs: CrossRefs) -> String {
        page(targets, HelpLanguage::English, cross_refs)
            .expect("a request the command shows builds a page")
            .markdown()
    }

    #[test]
    fn linked_pages_render_like_unlinked_pages() {
        for color in [false, true] {
            let documentation = Documentation::new(None, color);
            for targets in requests() {
                let linked = documentation.render(&markup(&targets, CrossRefs::Emit));
                let unlinked = documentation.render(&markup(&targets, CrossRefs::Suppress));
                assert_eq!(linked, unlinked, "{targets:?} color={color}");
            }
        }
    }

    /// A container with no page of its own is listed and shown as its sole child table.
    #[test]
    fn empty_container_reaches_its_sole_child() {
        assert_eq!(
            sole_child_table("typst").map(|table| table.header()),
            Some("[typst.fonts]".to_owned())
        );
        // A section that declares keys of its own keeps its page even when it reaches children.
        assert!(sole_child_table("build").is_none());
        let index = markup(&Vec::new(), CrossRefs::Emit);
        for section in ["build.minify", "build.references", "typst.fonts"] {
            assert!(index.contains(&format!("`[{section}]`")), "{index}");
        }
        assert!(!index.contains("`[typst]`"), "{index}");
        let page = request(&["[typst]"]).unwrap();
        assert!(
            page.starts_with("[typst.fonts] - configuration table"),
            "{page}"
        );
        // Every example command links the token it names, package and export alike.
        for uri in [
            "tola://table/[site]",
            "tola://package/@tola/web",
            "tola://selection/@tola/address:slugify,output-to-url",
            "tola://table/[build.hooks]",
            "tola://selection/@tola/document:headings,references",
            "tola://selection/@tola/code:code-themes",
        ] {
            assert!(index.contains(uri), "`{uri}` is not a link: {index}");
        }
    }
    #[test]
    fn every_cross_reference_resolves() {
        let mut loaded: Vec<(PageId, HelpDocument)> = Vec::new();
        for targets in requests() {
            let page = page(&targets, HelpLanguage::English, CrossRefs::Emit)
                .expect("a request the command shows builds a page");
            let document = HelpDocument::parse(page);
            for (block, target) in cross_references(&document) {
                match &target {
                    // A page may link out; only a malformed cross-reference is a failure.
                    LinkTarget::External(url) => {
                        assert!(
                            !url.starts_with(CROSS_REFERENCE_SCHEME),
                            "{targets:?} block {block}: `{url}` did not decode"
                        );
                    }
                    LinkTarget::Page(id) => {
                        load_page(id, &mut loaded);
                    }
                    LinkTarget::PageAnchor(id, anchor) => {
                        let target = load_page(id, &mut loaded);
                        assert!(
                            target.contains_anchor(anchor),
                            "{targets:?} block {block}: `{}` has no `{}`",
                            id.selector().unwrap_or_default(),
                            anchor.as_str()
                        );
                    }
                }
            }
        }
    }

    /// The document one id names, built and parsed at most once.
    fn load_page(id: &PageId, loaded: &mut Vec<(PageId, HelpDocument)>) -> HelpDocument {
        if let Some((_, document)) = loaded.iter().find(|(loaded, _)| loaded == id) {
            return document.clone();
        }
        let page = page_of(id, HelpLanguage::English, CrossRefs::Emit)
            .expect("a cross-reference names a page the command can show");
        let document = HelpDocument::parse(page);
        loaded.push((id.clone(), document.clone()));
        document
    }

    /// Every cross-reference one page holds, with the block it sits in.
    fn cross_references(document: &HelpDocument) -> Vec<(usize, LinkTarget)> {
        let mut found = Vec::new();
        for (block, content) in document.blocks.iter().enumerate() {
            walk_block(content, block, &mut found);
        }
        found
    }

    fn walk_block(block: &Block, at: usize, found: &mut Vec<(usize, LinkTarget)>) {
        match block {
            Block::Paragraph { spans, .. } | Block::Heading { spans, .. } => {
                walk_spans(spans, at, found)
            }
            Block::List { items, .. } => {
                for item in items {
                    for block in item {
                        walk_block(block, at, found);
                    }
                }
            }
            Block::Quote { blocks } => {
                for block in blocks {
                    walk_block(block, at, found);
                }
            }
            Block::Table { headers, rows } => {
                for spans in headers.iter().chain(rows.iter().flatten()) {
                    walk_spans(spans, at, found);
                }
            }
            Block::Code { .. } | Block::Rule => {}
        }
    }

    fn walk_spans(spans: &[Inline], at: usize, found: &mut Vec<(usize, LinkTarget)>) {
        for span in spans {
            match span {
                Inline::Link { label, target } => {
                    found.push((at, target.clone()));
                    walk_spans(label, at, found);
                }
                Inline::Emph(children) | Inline::Strong(children) => {
                    walk_spans(children, at, found);
                }
                Inline::Text(_) | Inline::Code(_) => {}
            }
        }
    }

    #[test]
    fn package_documentation_follows_the_help_language() {
        let text = request_in(HelpLanguage::SimplifiedChinese, &["@tola/site"]).unwrap();
        assert!(text.contains("解析自 `tola.toml`"), "{text}");
        assert!(text.contains("\nImport in Typst:\n"), "{text}");
        assert!(text.contains("\nExports:\n"), "{text}");
        let address = request_in(HelpLanguage::SimplifiedChinese, &["@tola/address"]).unwrap();
        assert!(address.contains("\nslugify - function\n"), "{address}");
    }

    #[test]
    fn related_targets_render_help_commands() {
        let chinese = request_in(
            HelpLanguage::SimplifiedChinese,
            &["@tola/source", "parse-sources"],
        )
        .unwrap();
        assert!(chinese.contains("\nRelated\n"), "{chinese}");
        assert!(
            chinese.contains(
                "- `all-sources`, `tola-meta` - `tola help` \"@tola/source\" `all-sources` `tola-meta`"
            ),
            "{chinese}"
        );
        assert!(
            chinese.contains("- `@tola/schema` - `tola help \"@tola/schema\"`"),
            "{chinese}"
        );
        let english = request(&["@tola/source", "parse-sources"]).unwrap();
        assert!(english.contains("\nRelated\n"), "{english}");
    }

    #[test]
    fn related_omits_rendered_exports() {
        let whole = request(&["@tola/icon"]).unwrap();
        assert!(!whole.contains("`icon-bytes`, `icon-url` - "), "{whole}");
        let selected = request(&["@tola/icon", "icon"]).unwrap();
        assert!(
            selected.contains(
                "- `icon-bytes`, `icon-url` - `tola help` \"@tola/icon\" `icon-bytes` `icon-url`"
            ),
            "{selected}"
        );
    }

    #[test]
    fn selectors_reject_non_table_fields() {
        for target in [
            "[site.title]",
            "[build.assets]",
            "[icons.collections.brand]",
            "@tola/host",
            "@tola/address:1.0.0",
        ] {
            let error = request(&[target]).unwrap_err();
            assert_eq!(
                tola_build::diagnostic::attached(&error).unwrap()[0].code,
                crate::codes::help::TARGET
            );
        }
    }

    #[test]
    fn grouped_exports_validate_before_output() {
        let error = request(&["@tola/address", "slugify", "not-an-export"]).unwrap_err();
        let diagnostics = tola_build::diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics[0].code, crate::codes::help::MEMBER);
        assert!(diagnostics[0].message.contains("not-an-export"));
    }

    #[test]
    fn site_page_names_the_extra_map() {
        let text = request(&["[site]"]).unwrap();
        assert!(text.contains("`[site.extra]`"));
    }

    #[test]
    fn configuration_templates_preserve_source() {
        for table in TABLES {
            let target = table.header();
            let shown = sole_child_table(table.section).unwrap_or(table);
            let text = request(&[&target]).unwrap();
            assert!(text.starts_with(&format!("{} - configuration table", shown.header())));
            let template = (shown.template)().unwrap();
            let indented = template
                .lines()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains(&indented), "{target}: {text}");
        }
    }

    #[test]
    fn package_reference_covers_every_export() {
        for package in tola_packages::builtin_packages() {
            let target = format!("@tola/{}", package.spec().name);
            let text = request(&[&target]).unwrap();
            let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(text.contains(&format!("#import \"{}\": *", package.spec())));
            let names = package.exports().map(str::to_owned).collect::<Vec<_>>();
            for export in package.export_documentation(&names).unwrap() {
                assert!(text.contains(&format!("`{}` - ", export.name)), "{text}");
                assert!(text.contains(&format!("\n{} - ", export.name)), "{text}");
                let summary = export_summary(&export)
                    .unwrap_or_else(|| panic!("{} publishes no summary", export.name));
                assert!(words.contains(summary), "{text}");
                let lines = declaration(&export.declaration)
                    .lines()
                    .map(|line| format!("  {line}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(text.contains(&lines), "{}: {text}", export.name);
            }
        }
    }

    /// The signature one bundled export publishes, through the same model the command renders.
    fn export_signature(package: tola_packages::TolaPackage, export: &str) -> Signature {
        let package = tola_packages::builtin_package(&package.spec()).unwrap();
        let exports = package.export_documentation(&[export.to_owned()]).unwrap();
        let ExportDeclaration::Function(signature) =
            exports.into_iter().next().unwrap().declaration
        else {
            panic!("`{export}` should be a function")
        };
        signature
    }

    #[test]
    fn help_prints_the_signature_block_verbatim() {
        let text = request(&["@tola/address", "route"]).unwrap();
        let block = export_signature(tola_packages::TolaPackage::Address, "route").code();
        let indented = block
            .lines()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(&indented), "{text}");
    }

    #[test]
    fn selected_exports_keep_request_order() {
        let narrowed = request(&["@tola/address", "output-to-url", "slugify"]).unwrap();
        assert!(!narrowed.contains("\nExports:\n"), "{narrowed}");
        assert!(narrowed.contains("#import \"@tola/address:0.0.0\": output-to-url, slugify"));
        assert!(narrowed.contains("\nslugify - function\n"), "{narrowed}");
        assert!(
            narrowed.find("\noutput-to-url - function").unwrap()
                < narrowed.find("\nslugify - function").unwrap()
        );
        assert!(!narrowed.contains("\nroute - function"));
        for names in [
            ["code-themes", "code-stylesheet"],
            ["code-stylesheet", "code-themes"],
        ] {
            let targets = std::iter::once("@tola/code")
                .chain(names)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let document = HelpDocument::parse(
                page(&targets, HelpLanguage::English, CrossRefs::Emit).unwrap(),
            );
            let primary = document
                .blocks
                .iter()
                .filter_map(|block| match block {
                    Block::Heading {
                        anchor,
                        role: crate::help::model::HeadingRole::Export,
                        ..
                    } => Some(anchor.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let package =
                tola_packages::builtin_package(&tola_packages::TolaPackage::Code.spec()).unwrap();
            let names = names.map(str::to_owned);
            let expected = package
                .export_documentation(&names)
                .unwrap()
                .iter()
                .map(export_anchor)
                .collect::<Vec<_>>();
            assert_eq!(primary, expected);
        }
    }

    /// One export reads as its own block: the declaration, then its parameters, then what it is
    /// for, and the next export follows.
    #[test]
    fn export_sections_read_signature_parameters_then_prose() {
        let text = request(&["@tola/address", "slugify", "output-to-url"]).unwrap();
        let declaration = text.find("let slugify(").expect("the declaration");
        let parameters = text.find("Positional Parameters").expect("the parameters");
        let prose = text
            .find("Turn text into a URL-safe name")
            .expect("the prose");
        assert!(
            declaration < parameters && parameters < prose,
            "the export does not read declaration, parameters, prose:\n{text}"
        );
        assert!(
            prose < text.find("output-to-url - function").unwrap(),
            "the second export follows the first:\n{text}"
        );
    }

    #[test]
    fn declaration_fences_preserve_backticks() {
        let source = "sample(value: \"```\")\n  continuation";
        let rendered =
            Documentation::new(Some(24), false).render(&code_block("typst-code", source));
        assert!(rendered.contains("  sample(value: \"```\")\n    continuation"));
    }
}
