//! Read-only help pages assembled from configuration, packages, and embedded demos.

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

use crate::config::{DevConfig, DiagnosticsConfig, ServerConfig};
use crate::help::model::{Anchor, HelpCategory, HelpPage, LinkTarget, PageId, anchor};
use crate::i18n::{HelpLanguage, HelpText, PackageTranslations};
use crate::terminal::documentation::Documentation;

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

/// Whether a page writes its cross-references as links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CrossRefs {
    /// Production: a code span that names another page holds its `tola-help://` reference.
    Emit,
    /// The same page without links, so a test can prove the links add no visible text.
    #[allow(dead_code)] // Production emits links; the tests compare both modes.
    Suppress,
}

pub(crate) fn request(targets: &[String]) -> Result<LinkTarget> {
    super::target::request(targets).ok_or_else(|| {
        target_error(
            &targets.join(" "),
            "Choose `tola help config`, `tola help package`, or `tola help demo`",
        )
    })
}

fn page(targets: &[String], language: HelpLanguage, cross_refs: CrossRefs) -> Result<HelpPage> {
    let target = request(targets)?;
    page_of(
        target.page().expect("a help request locates a page"),
        language,
        cross_refs,
    )
}

pub(crate) fn page_of(
    id: &PageId,
    language: HelpLanguage,
    cross_refs: CrossRefs,
) -> Result<HelpPage> {
    match id {
        PageId::Overview => Ok(overview_page(language, cross_refs)),
        PageId::Index(category) => Ok(index_page(*category, language, cross_refs)),
        PageId::Config { section } => table_page(section, language, cross_refs),
        PageId::Package { name } => package_page(name, &[], language, cross_refs),
        PageId::PackageSelection { name, exports } => {
            package_page(name, exports, language, cross_refs)
        }
        PageId::Demo { id } => demo_page(id, language, cross_refs),
        PageId::DemoFile { id, path } => demo_file_page(id, path, language),
        PageId::DemoOutputs { id } => output_page(id, None, None, language, cross_refs),
        PageId::DemoOutput { id, path } => output_page(id, Some(path), None, language, cross_refs),
    }
}

/// One named token, linked to the page it names when the page emits cross-references.
///
/// Only the token is linked: a link adds no styling of its own, so the page renders the same
/// bytes with and without its links, and the plain renderer hides the reserved target. The
/// command around the token stays literal text, so the quoting the reader copies stays intact.
fn cross_reference(token: &str, target: &LinkTarget, cross_refs: CrossRefs) -> String {
    match cross_refs {
        CrossRefs::Emit => format!("[`{token}`]({})", target.uri()),
        CrossRefs::Suppress => format!("`{token}`"),
    }
}

/// One named token, linked to the page it names when the index can resolve it; a mention the
/// index cannot resolve stays plain text, because the index outlives a rename.
fn named(token: &str, target: Option<LinkTarget>, cross_refs: CrossRefs) -> String {
    match target {
        Some(target) => cross_reference(token, &target, cross_refs),
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

pub(crate) fn render(
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

fn overview_page(language: HelpLanguage, cross_refs: CrossRefs) -> HelpPage {
    let id = PageId::Overview;
    let text = |phrase| crate::i18n::text(language, phrase);
    let mut markdown = format!(
        "# Tola help\n\n{}\n\n{}\n",
        text(HelpText::SiteModel),
        text(HelpText::HelpUsage)
    );
    for (title, category, command) in [
        ("Configuration", HelpCategory::Config, "config"),
        ("Packages", HelpCategory::Packages, "package"),
        ("Demos", HelpCategory::Demos, "demo"),
    ] {
        let linked = cross_reference(
            title,
            &LinkTarget::Page(PageId::Index(category)),
            cross_refs,
        );
        markdown.push_str(&format!("\n- {linked} — `tola help {command}`\n"));
    }
    markdown.push_str("\n## Find a topic\n\n");
    for (name, exports, phrase) in [
        (
            "source",
            &["tola-meta", "all-sources", "parse-sources"][..],
            HelpText::SourcePages,
        ),
        (
            "address",
            &["route-to-output", "output-to-url", "asset-url"][..],
            HelpText::PageAddresses,
        ),
        (
            "document",
            &["headings", "references"][..],
            HelpText::DocumentQueries,
        ),
        ("web", &["feed", "sitemap"][..], HelpText::WebOutputs),
    ] {
        let command = format!("tola help package {name} {}", exports.join(" "));
        markdown.push_str(&format!(
            "- {} — {}\n",
            named(&command, package_target(name, exports), cross_refs),
            text(phrase)
        ));
    }
    markdown.push_str(&format!(
        "- {} — {}\n",
        named("tola help config build", table_target("build"), cross_refs),
        text(HelpText::BuildPaths)
    ));
    markdown.push_str(&format!(
        "- {} — {}\n",
        named(
            "tola help config build.hooks",
            table_target("build.hooks"),
            cross_refs
        ),
        text(HelpText::BuildHooks)
    ));
    HelpPage::new(id, markdown)
}

fn index_page(category: HelpCategory, language: HelpLanguage, cross_refs: CrossRefs) -> HelpPage {
    let id = PageId::Index(category);
    let mut markdown = String::new();
    match category {
        HelpCategory::Config => {
            markdown.push_str("# Configuration tables\n\n");
            for table in TABLES
                .iter()
                .filter(|table| !is_empty_container(table.section))
            {
                let shown = sole_child_table(table.section).unwrap_or(table);
                let label = named(&shown.header(), table_target(shown.section), cross_refs);
                let command = PageId::Config {
                    section: shown.section.into(),
                }
                .selector()
                .expect("a configuration page has CLI arguments");
                markdown.push_str(&format!("- {label} — `tola help {command}`\n"));
            }
        }
        HelpCategory::Packages => {
            markdown.push_str("# Bundled packages\n\n");
            for package in tola_packages::builtin_packages() {
                let name = package.spec().name.to_string();
                let label = named(
                    &format!("@tola/{name}"),
                    package_target(&name, &[]),
                    cross_refs,
                );
                let command = PageId::Package { name }
                    .selector()
                    .expect("a package page has CLI arguments");
                markdown.push_str(&format!("- {label} — `tola help {command}`\n"));
            }
        }
        HelpCategory::Demos => {
            markdown.push_str(&format!(
                "# Demos\n\n{}\n\n",
                crate::i18n::text(language, HelpText::DemoUsage)
            ));
            for demo in crate::demos::all() {
                let target = LinkTarget::Page(PageId::Demo { id: demo.id.into() });
                let title = cross_reference(demo.title(language), &target, cross_refs);
                let command = PageId::Demo { id: demo.id.into() }
                    .selector()
                    .expect("a demo has CLI arguments");
                markdown.push_str(&format!(
                    "- {title} — {} — `tola help {command}`\n",
                    demo.summary(language)
                ));
            }
        }
    }
    HelpPage::new(id, markdown)
}

pub(crate) fn label(id: &PageId, language: HelpLanguage) -> String {
    match id {
        PageId::Overview => "tola help".into(),
        PageId::Index(HelpCategory::Config) => "Configuration".into(),
        PageId::Index(HelpCategory::Packages) => "Packages".into(),
        PageId::Index(HelpCategory::Demos) => "Demos".into(),
        PageId::Config { section } => TABLES
            .iter()
            .find(|table| table.section == section)
            .map(ConfigTable::header)
            .unwrap_or_else(|| format!("[{section}]")),
        PageId::Package { name } => format!("@tola/{name}"),
        PageId::PackageSelection { name, exports } => format!("@tola/{name} {}", exports.join(" ")),
        PageId::Demo { id } => crate::demos::find(id)
            .map(|demo| demo.title(language))
            .unwrap_or(id)
            .into(),
        PageId::DemoFile { id, path } => format!(
            "{} · {path}",
            label(&PageId::Demo { id: id.clone() }, language)
        ),
        PageId::DemoOutputs { id } => format!(
            "{} · Outputs",
            label(&PageId::Demo { id: id.clone() }, language)
        ),
        PageId::DemoOutput { id, path } => format!(
            "{} · {path}",
            label(&PageId::Demo { id: id.clone() }, language)
        ),
    }
}

/// The table page one section names, when the index names a real table.
fn table_target(section: &str) -> Option<LinkTarget> {
    TABLES
        .iter()
        .find(|table| table.section == section)
        .map(|table| {
            LinkTarget::Page(PageId::Config {
                section: table.section.to_owned(),
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
    let name = name.to_owned();
    Some(LinkTarget::Page(match exports {
        [] => PageId::Package { name },
        exports => PageId::PackageSelection {
            name,
            exports: exports.iter().map(|export| (*export).to_owned()).collect(),
        },
    }))
}

fn table_page(target: &str, language: HelpLanguage, cross_refs: CrossRefs) -> Result<HelpPage> {
    let table = TABLES
        .iter()
        .find(|table| table.section == target)
        .ok_or_else(|| target_error(target, &table_choices()))?;
    // A container whose only content is its one child shows that child's page instead, so
    // the reader lands on the table that names real settings.
    if let Some(child) = sole_child_table(table.section) {
        return table_page(child.section, language, cross_refs);
    }
    let id = PageId::Config {
        section: table.section.to_owned(),
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
            let child_id = PageId::Config {
                section: child.section.to_owned(),
            };
            let header = child.header();
            // A table header is an array when its declaration is, so it includes its own brackets.
            let table = cross_reference(&header, &LinkTarget::Page(child_id), cross_refs);
            markdown.push_str(&format!(
                "- {table} — `tola help config {}`\n",
                child.section
            ));
        }
    } else {
        markdown.push_str(&format!(
            "\n{}\n\n",
            crate::i18n::text(language, HelpText::DefaultValues)
        ));
        markdown.push_str(&code_block("toml", &template));
        markdown.push_str(&declared_keys(table.section, translations, cross_refs));
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
            cross_refs,
        );
        markdown.push_str(&format!("\n## Map-shaped {heading}\n\n{docs}\n"));
    }
    append_demos(&mut markdown, language, cross_refs, |demo| {
        demo.related_table(table.section)
    });
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
                &LinkTarget::Page(PageId::Config {
                    section: child.section.to_owned(),
                }),
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
    name: &str,
    requested: &[String],
    language: HelpLanguage,
    cross_refs: CrossRefs,
) -> Result<HelpPage> {
    let package = tola_packages::builtin_packages()
        .find(|package| package.spec().name == name)
        .ok_or_else(|| target_error(name, &package_choices()))?;
    let target = format!("@tola/{name}");
    for member in requested {
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
    let whole_package = requested.is_empty();
    let names = if whole_package {
        package.exports().map(str::to_owned).collect::<Vec<_>>()
    } else {
        requested.to_vec()
    };
    let imports = if whole_package {
        "*".to_owned()
    } else {
        names.join(", ")
    };
    let id = if whole_package {
        PageId::Package {
            name: name.to_owned(),
        }
    } else {
        PageId::PackageSelection {
            name: name.to_owned(),
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
            "\nRead selected exports: `tola help package {name}{example}`\n"
        ));
    }
    append_demos(&mut text, language, cross_refs, |demo| {
        demo.related_package(name, requested)
    });
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
            &target,
            &export.related,
            &names,
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
            name: target.trim_start_matches("@tola/").to_owned(),
            exports: exports.iter().map(|name| (*name).to_owned()).collect(),
        };
        let named_exports = exports
            .iter()
            .map(|name| cross_reference(name, &LinkTarget::Page(selection.clone()), cross_refs))
            .collect::<Vec<_>>()
            .join(" ");
        text.push_str(&format!(
            "- {listed} - `tola help package {}` {named_exports}\n",
            target.trim_start_matches("@tola/")
        ));
    }
    for package in packages {
        let name = cross_reference(
            &format!("@tola/{package}"),
            &LinkTarget::Page(PageId::Package {
                name: package.to_owned(),
            }),
            cross_refs,
        );
        text.push_str(&format!("- {name} - `tola help package {package}`\n"));
    }
    text
}

fn append_demos(
    markdown: &mut String,
    language: HelpLanguage,
    cross_refs: CrossRefs,
    related: impl Fn(&crate::demos::Demo) -> bool,
) {
    let demos = crate::demos::all()
        .iter()
        .filter(|demo| related(demo))
        .collect::<Vec<_>>();
    if demos.is_empty() {
        return;
    }
    markdown.push_str("\n## Demos\n\n");
    for demo in demos {
        let target = LinkTarget::Page(PageId::Demo { id: demo.id.into() });
        let title = cross_reference(demo.title(language), &target, cross_refs);
        markdown.push_str(&format!("- {title} — {}\n", demo.summary(language)));
    }
}

pub(crate) fn demo(id: &str) -> Result<&'static crate::demos::Demo> {
    crate::demos::find(id).ok_or_else(|| target_error(id, "Use `tola help demo` to list the demos"))
}

fn demo_page(id: &str, language: HelpLanguage, cross_refs: CrossRefs) -> Result<HelpPage> {
    let demo = demo(id)?;
    let page = PageId::Demo { id: id.into() };
    let mut markdown = demo.documentation(language)?;
    link_source_headings(&mut markdown, demo, cross_refs);
    markdown.push_str(&format!(
        "\n## Read source files\n\n{}\n\n",
        crate::i18n::text(language, HelpText::DemoSource)
    ));
    for file in demo.files() {
        let target = LinkTarget::Page(PageId::DemoFile {
            id: id.into(),
            path: file.path.into(),
        });
        markdown.push_str(&format!(
            "- {}\n",
            cross_reference(file.path, &target, cross_refs)
        ));
    }
    markdown.push_str(&format!(
        "\n{}\n",
        crate::i18n::text(language, HelpText::DemoActions)
    ));
    Ok(HelpPage::new(page, markdown))
}

fn link_source_headings(markdown: &mut String, demo: &crate::demos::Demo, cross_refs: CrossRefs) {
    let headings = pulldown_cmark::Parser::new(markdown)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(
                event,
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::Heading { .. })
            )
            .then_some(range)
        })
        .collect::<Vec<_>>();
    // Backward edits preserve the parser's offsets, and fenced source never becomes a heading.
    for range in headings.into_iter().rev() {
        let heading = markdown[range.clone()].trim_end();
        if let Some(file) = demo
            .files()
            .iter()
            .find(|file| heading == format!("### `{}`", file.path))
        {
            let target = LinkTarget::Page(PageId::DemoFile {
                id: demo.id.into(),
                path: file.path.into(),
            });
            let linked = format!("### {}\n", cross_reference(file.path, &target, cross_refs));
            markdown.replace_range(range, &linked);
        }
    }
}

pub(crate) fn source_file(id: &str, path: &str) -> Result<&'static crate::demos::DemoFile> {
    demo(id)?.file(path).ok_or_else(|| {
        help_error(
            crate::codes::help::TARGET,
            format!("`{path}` is not a source file in demo `{id}`"),
            format!("Use `tola help demo {id}` to choose a file"),
        )
    })
}

fn demo_file_page(id: &str, path: &str, language: HelpLanguage) -> Result<HelpPage> {
    let file = source_file(id, path)?;
    let page = PageId::DemoFile {
        id: id.into(),
        path: path.into(),
    };
    let mut markdown = format!(
        "# {}\n\n{}\n\n",
        label(&page, language),
        crate::i18n::text(language, HelpText::DemoSource)
    );
    markdown.push_str(&file_content(path, file.bytes, source_media_type(path)));
    Ok(HelpPage::new(page, markdown))
}

pub(crate) fn output_page(
    id: &str,
    path: Option<&str>,
    ready: Option<&crate::demos::preview::PreviewReady>,
    language: HelpLanguage,
    cross_refs: CrossRefs,
) -> Result<HelpPage> {
    demo(id)?;
    let page = match path {
        Some(path) => PageId::DemoOutput {
            id: id.into(),
            path: path.into(),
        },
        None => PageId::DemoOutputs { id: id.into() },
    };
    let mut markdown = format!("# {}\n\n", label(&page, language));
    let Some(ready) = ready else {
        markdown.push_str(crate::i18n::text(language, HelpText::DemoOutputs));
        markdown.push('\n');
        return Ok(HelpPage::new(page, markdown));
    };
    if let Some(path) = path {
        let output = ready
            .outputs
            .iter()
            .find(|output| output.path == path)
            .ok_or_else(|| {
                help_error(
                    crate::codes::help::TARGET,
                    format!("`{path}` is not an output of demo `{id}`"),
                    "Choose a file from the demo's output list".into(),
                )
            })?;
        markdown.push_str(&file_content(
            &output.path,
            &output.bytes,
            &output.media_type,
        ));
        let uri = output_url(&ready.url, &output.path)?;
        markdown.push_str(&format!("\n[Open in browser]({uri})\n"));
    } else {
        markdown.push_str("| Output | Media type | Bytes |\n| --- | --- | --- |\n");
        for output in ready.outputs.iter() {
            let target = LinkTarget::Page(PageId::DemoOutput {
                id: id.into(),
                path: output.path.clone(),
            });
            let file = cross_reference(&output.path, &target, cross_refs);
            markdown.push_str(&format!(
                "| {file} | `{}` | {} |\n",
                output.media_type,
                output.bytes.len()
            ));
        }
    }
    Ok(HelpPage::new(page, markdown))
}

pub(crate) fn output_url(landing: &str, path: &str) -> Result<String> {
    let output = tola_address::OutputPath::parse(path)?;
    let mut uri = url::Url::parse(landing)?;
    uri.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("the demo has no browser address"))?
        .pop_if_empty()
        .extend(output.as_str().split('/'));
    Ok(uri.into())
}

fn file_content(path: &str, bytes: &[u8], media_type: &str) -> String {
    let mut markdown = format!("`{media_type}` · {} bytes\n\n", bytes.len());
    let text_type = media_type.starts_with("text/")
        || media_type.ends_with("+xml")
        || matches!(
            media_type,
            "application/xml" | "application/json" | "application/javascript"
        );
    if text_type && let Ok(source) = std::str::from_utf8(bytes) {
        markdown.push_str(&code_block(source_language(path), source));
    } else {
        markdown.push_str(
            "Binary file. Preview the demo to open its published output in the browser.\n",
        );
    }
    markdown
}

fn source_language(path: &str) -> &str {
    match path.rsplit('.').next().unwrap_or_default() {
        "typ" => "typst",
        "toml" => "toml",
        "css" => "css",
        "js" => "javascript",
        "json" => "json",
        "html" => "html",
        "svg" | "xml" => "xml",
        _ => "text",
    }
}

fn source_media_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "html" => "text/html",
        "xml" => "application/xml",
        _ => "text/plain; charset=utf-8",
    }
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

fn table_choices() -> String {
    format!(
        "Configuration tables: {}",
        config_sections().collect::<Vec<_>>().join(", ")
    )
}

fn package_choices() -> String {
    format!(
        "Bundled packages: {}",
        package_names().collect::<Vec<_>>().join(", ")
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
fn config_sections() -> impl Iterator<Item = String> {
    TABLES.iter().map(|table| table.section.to_owned())
}

/// Every bundled-package selector, shared by the no-argument index and the error help.
fn package_names() -> impl Iterator<Item = String> {
    tola_packages::builtin_packages().map(|package| package.spec().name.to_string())
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
    use crate::help::model::{Block, CROSS_REFERENCE_SCHEME, HelpDocument, Inline};

    fn targets(arguments: &[&str]) -> Vec<String> {
        arguments
            .iter()
            .map(|argument| (*argument).into())
            .collect()
    }

    fn requests() -> Vec<Vec<String>> {
        let mut requests = vec![
            vec![],
            targets(&["config"]),
            targets(&["package"]),
            targets(&["demo"]),
        ];
        requests.extend(
            TABLES
                .iter()
                .map(|table| targets(&["config", table.section])),
        );
        for package in tola_packages::builtin_packages() {
            let name = package.spec().name.to_string();
            requests.push(targets(&["package", &name]));
            let mut selected = targets(&["package", &name]);
            selected.extend(package.exports().take(2).map(str::to_owned));
            requests.push(selected);
        }
        for demo in crate::demos::all() {
            requests.push(targets(&["demo", demo.id]));
            for file in demo.files() {
                requests.push(targets(&["demo", demo.id, file.path]));
            }
        }
        requests
    }

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

    #[test]
    fn links_preserve_visible_documentation() {
        for color in [false, true] {
            let renderer = Documentation::new(None, color);
            for arguments in requests() {
                let linked = page(&arguments, HelpLanguage::English, CrossRefs::Emit).unwrap();
                let unlinked =
                    page(&arguments, HelpLanguage::English, CrossRefs::Suppress).unwrap();
                assert_eq!(
                    renderer.render(&linked.markdown()),
                    renderer.render(&unlinked.markdown()),
                    "{arguments:?}"
                );
            }
        }
    }

    #[test]
    fn empty_container_shows_child_settings() {
        let page = page(
            &targets(&["config", "typst"]),
            HelpLanguage::English,
            CrossRefs::Emit,
        )
        .unwrap();
        assert_eq!(
            page.id,
            PageId::Config {
                section: "typst.fonts".into()
            }
        );
        assert!(sole_child_table("build").is_none());
    }

    #[test]
    fn help_links_resolve_registered_pages() {
        let mut loaded: Vec<(PageId, HelpDocument)> = Vec::new();
        for arguments in requests() {
            let document = HelpDocument::parse(
                page(&arguments, HelpLanguage::English, CrossRefs::Emit).unwrap(),
            );
            for (_, target) in cross_references(&document) {
                match &target {
                    LinkTarget::External(uri) => {
                        assert!(!uri.starts_with(CROSS_REFERENCE_SCHEME), "{uri}")
                    }
                    LinkTarget::Page(id) | LinkTarget::PageAnchor(id, _) => {
                        let document = if let Some((_, document)) =
                            loaded.iter().find(|(page, _)| page == id)
                        {
                            document.clone()
                        } else {
                            let document = HelpDocument::parse(
                                page_of(id, HelpLanguage::English, CrossRefs::Emit).unwrap(),
                            );
                            loaded.push((id.clone(), document.clone()));
                            document
                        };
                        if let LinkTarget::PageAnchor(_, anchor) = &target {
                            assert!(
                                document.contains_anchor(anchor),
                                "{}#{}",
                                id.uri(),
                                anchor.as_str()
                            );
                        }
                    }
                }
            }
        }
    }

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
    fn selected_exports_preserve_order() {
        let selected = targets(&["package", "address", "output-to-url", "slugify"]);
        let document =
            HelpDocument::parse(page(&selected, HelpLanguage::English, CrossRefs::Emit).unwrap());
        let names = document
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Heading {
                    spans,
                    role: crate::help::model::HeadingRole::Export,
                    ..
                } => spans.first().and_then(|span| match span {
                    Inline::Text(text) | Inline::Code(text) => {
                        text.split_whitespace().next().map(str::to_owned)
                    }
                    _ => None,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["output-to-url", "slugify"]);
    }

    #[test]
    fn unknown_exports_prevent_output() {
        let error = page(
            &targets(&["package", "address", "slugify", "unknown"]),
            HelpLanguage::English,
            CrossRefs::Emit,
        )
        .err()
        .expect("an unknown export is rejected");
        assert_eq!(
            tola_build::diagnostic::attached(&error).unwrap()[0].code,
            crate::codes::help::MEMBER
        );
    }

    #[test]
    fn demos_follow_related_exports() {
        let related = page(
            &targets(&["package", "document", "references"]),
            HelpLanguage::English,
            CrossRefs::Emit,
        )
        .unwrap();
        let references = cross_references(&HelpDocument::parse(related));
        assert!(references.iter().any(|(_, target)| target
            == &LinkTarget::Page(PageId::Demo {
                id: "backlinks".into()
            })));
        assert!(
            !references
                .iter()
                .any(|(_, target)| target == &LinkTarget::Page(PageId::Demo { id: "toc".into() }))
        );
        let table = page(
            &targets(&["config", "assets"]),
            HelpLanguage::English,
            CrossRefs::Emit,
        )
        .unwrap();
        assert!(cross_references(&HelpDocument::parse(table)).iter().any(|(_, target)| target == &LinkTarget::Page(PageId::Demo { id: "media".into() })));
    }

    #[test]
    fn source_views_preserve_complete_files() {
        for demo in crate::demos::all() {
            for file in demo.files() {
                let page = demo_file_page(demo.id, file.path, HelpLanguage::English).unwrap();
                let document = HelpDocument::parse(page);
                let code = document.blocks.iter().find_map(|block| match block {
                    Block::Code { source, .. } => Some(source.as_str()),
                    _ => None,
                });
                match std::str::from_utf8(file.bytes) {
                    Ok(source) => {
                        let expected = if source.ends_with('\n') {
                            source.to_owned()
                        } else {
                            format!("{source}\n")
                        };
                        assert_eq!(code, Some(expected.as_str()));
                    }
                    Err(_) => assert!(code.is_none()),
                }
            }
        }
    }

    #[test]
    fn output_views_use_ready_bytes() {
        let ready = crate::demos::preview::PreviewReady {
            url: "http://127.0.0.1:1234/docs/".into(),
            outputs: std::sync::Arc::from([
                crate::demos::preview::DemoOutput {
                    path: "feed.xml".into(),
                    media_type: "application/rss+xml".into(),
                    bytes: bytes::Bytes::from_static(b"<rss>content</rss>"),
                },
                crate::demos::preview::DemoOutput {
                    path: "download.bin".into(),
                    media_type: "application/octet-stream".into(),
                    bytes: bytes::Bytes::from_static(b"binary"),
                },
            ]),
        };
        let text = HelpDocument::parse(
            output_page(
                "feeds",
                Some("feed.xml"),
                Some(&ready),
                HelpLanguage::English,
                CrossRefs::Emit,
            )
            .unwrap(),
        );
        assert!(text.blocks.iter().any(|block| matches!(block, Block::Code { source, .. } if source.trim_end_matches('\n') == "<rss>content</rss>")));
        let binary = HelpDocument::parse(
            output_page(
                "feeds",
                Some("download.bin"),
                Some(&ready),
                HelpLanguage::English,
                CrossRefs::Emit,
            )
            .unwrap(),
        );
        assert!(
            !binary
                .blocks
                .iter()
                .any(|block| matches!(block, Block::Code { .. }))
        );
        assert_eq!(
            output_url(&ready.url, "100% notes.html").unwrap(),
            "http://127.0.0.1:1234/docs/100%25%20notes.html"
        );
    }

    #[test]
    fn tutorial_links_preserve_fenced_source() {
        let demo = crate::demos::find("backlinks").unwrap();
        let source = "### `site/page.typ`\n";
        let mut markdown = format!("{source}\n```text\n{source}```\n");
        let id = PageId::Demo { id: demo.id.into() };
        link_source_headings(&mut markdown, demo, CrossRefs::Emit);
        let document = HelpDocument::parse(HelpPage::new(id.clone(), markdown));
        assert!(
            document.blocks.iter().any(
                |block| matches!(block, Block::Code { source: actual, .. } if actual == source)
            )
        );
        assert!(cross_references(&document).iter().any(|(_, target)| target
            == &LinkTarget::Page(PageId::DemoFile {
                id: demo.id.into(),
                path: "site/page.typ".into()
            })));
    }
}
