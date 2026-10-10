//! Read-only help pages assembled from configuration and packages.

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

struct ConfigTable {
    section: &'static str,
    array: bool,
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
    },
    ConfigTable {
        section: BuildSectionConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: MinifyConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: ReferencesConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: HooksConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: BeforeBuildHookConfig::TEMPLATE_SECTION,
        array: true,
    },
    ConfigTable {
        section: OutputCommandConfig::TEMPLATE_SECTION,
        array: true,
    },
    ConfigTable {
        section: AfterPublishHookConfig::TEMPLATE_SECTION,
        array: true,
    },
    ConfigTable {
        section: AssetsConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: TypstSectionConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: FontsConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: IconsConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: VendorConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: ServerConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: DevConfig::TEMPLATE_SECTION,
        array: false,
    },
    ConfigTable {
        section: DiagnosticsConfig::TEMPLATE_SECTION,
        array: false,
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
            "Choose `tola help config` or `tola help package`",
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
        PageId::Index(category) => Ok(index_page(*category, cross_refs)),
        PageId::Config { section } => table_page(section, language, cross_refs),
        PageId::Package { name } => package_page(name, &[], language, cross_refs),
        PageId::PackageSelection { name, exports } => {
            package_page(name, exports, language, cross_refs)
        }
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
    let mut markdown = format!("# Tola help\n\n{}\n", text(HelpText::HelpUsage));
    for (title, category) in [
        ("Configuration", HelpCategory::Config),
        ("Packages", HelpCategory::Packages),
    ] {
        let linked = cross_reference(
            title,
            &LinkTarget::Page(PageId::Index(category)),
            cross_refs,
        );
        markdown.push_str(&format!("\n- {linked}\n"));
    }
    markdown.push_str("\n## Examples\n\n");
    for (name, exports) in [
        ("source", &["tola-meta", "all-sources", "parse-sources"][..]),
        ("address", &["output-of", "url-of", "url-of-asset"][..]),
        ("document", &["headings", "references"][..]),
        ("web", &["feed", "sitemap"][..]),
    ] {
        let command = format!("tola help \"@tola/{name}\" {}", exports.join(" "));
        markdown.push_str(&format!(
            "- {}\n",
            named(&command, package_target(name, exports), cross_refs)
        ));
    }
    markdown.push_str(&format!(
        "- {}\n",
        named("tola help \"[build]\"", table_target("build"), cross_refs)
    ));
    markdown.push_str(&format!(
        "- {}\n",
        named(
            "tola help \"[build.hooks]\"",
            table_target("build.hooks"),
            cross_refs
        )
    ));
    HelpPage::new(id, markdown)
}

fn index_page(category: HelpCategory, cross_refs: CrossRefs) -> HelpPage {
    let id = PageId::Index(category);
    let mut markdown = String::new();
    match category {
        HelpCategory::Config => {
            markdown.push_str("# Configuration tables\n\n");
            // A table with settings of its own is an entry; so is a container grouping several
            // child tables, such as `[build.hooks]`.
            for table in TABLES.iter().filter(|table| {
                has_own_key(table.section) || child_tables(table.section).count() > 1
            }) {
                let label = named(&table.header(), table_target(table.section), cross_refs);
                markdown.push_str(&format!("- {label}\n"));
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
                markdown.push_str(&format!("- {label}\n"));
            }
        }
    }
    HelpPage::new(id, markdown)
}

pub(crate) fn label(id: &PageId) -> String {
    match id {
        PageId::Overview => "tola help".into(),
        PageId::Index(HelpCategory::Config) => "Configuration".into(),
        PageId::Index(HelpCategory::Packages) => "Packages".into(),
        PageId::Config { section } => table_header(section),
        PageId::Package { name } => format!("@tola/{name}"),
        PageId::PackageSelection { name, exports } => format!("@tola/{name} {}", exports.join(" ")),
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
    let mut markdown = format!("# {} - configuration table\n", table.header());
    let translations = crate::i18n::sections(language);
    if let Some(documentation) = documented(table.section).map(|english| {
        translations
            .and_then(|tables| tables.documentation(table.section))
            .unwrap_or(english)
    }) {
        markdown.push_str(&format!("\n{documentation}\n"));
    }
    if has_own_key(table.section) {
        markdown.push_str(&declared_keys(table.section, translations, cross_refs));
    } else {
        markdown.push_str("\nConfigure this section through its child tables:\n");
        for child in child_tables(table.section) {
            let child_id = PageId::Config {
                section: child.section.to_owned(),
            };
            let header = child.header();
            // A table header is an array when its declaration is, so it includes its own brackets.
            let table = cross_reference(&header, &LinkTarget::Page(child_id), cross_refs);
            markdown.push_str(&format!(
                "- {table} - {}\n",
                child_description(child, translations)
            ));
        }
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

    let packages = related_packages(table.section);
    if !packages.is_empty() {
        markdown.push_str("\n### Related\n\n");
        for package in packages {
            let name = cross_reference(
                &format!("@tola/{package}"),
                &LinkTarget::Page(PageId::Package {
                    name: package.to_owned(),
                }),
                cross_refs,
            );
            markdown.push_str(&format!("- {name}\n"));
        }
    }
    Ok(HelpPage::new(id, markdown))
}

/// Every key this section's declaration writes, in schema order, without the nested ones: its
/// settings, and the keys opening child tables, each rendered as one bullet. The map-shaped
/// `[site.extra]` is explained by the table that holds it.
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

/// The one rendered line describing one child table: the documentation its own page opens with.
fn child_description(
    child: &ConfigTable,
    translations: Option<&crate::i18n::SectionTranslations>,
) -> String {
    let english = documented(child.section).expect("every table documents itself");
    one_line(
        translations
            .and_then(|tables| tables.documentation(child.section))
            .unwrap_or(english),
    )
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
    !has_own_key(section)
}

/// Whether a table writes a key of its own: one that names a setting instead of opening a child
/// table.
fn has_own_key(section: &str) -> bool {
    documented_fields(section)
        .iter()
        .any(|path| child_table(path).is_none())
}

/// The one child table `section` reaches, when a container has nothing but it.
fn sole_child_table(section: &str) -> Option<&'static ConfigTable> {
    is_empty_container(section)
        .then(|| child_tables(section).next())
        .flatten()
}

/// Every key the section's own declaration writes, one bullet each: a setting with its
/// documentation, a child table with the description its own page opens with.
///
/// The meaning is the declaration's documentation, and one key is one rendered line: the page
/// wraps it.
fn declared_keys(
    section: &str,
    translations: Option<&crate::i18n::SectionTranslations>,
    cross_refs: CrossRefs,
) -> String {
    let mut markdown = String::new();
    for path in documented_fields(section) {
        if let Some(child) = child_table(path) {
            let header = cross_reference(
                &child.header(),
                &LinkTarget::Page(PageId::Config {
                    section: child.section.to_owned(),
                }),
                cross_refs,
            );
            markdown.push_str(&format!(
                "\n- {header} - {}",
                child_description(child, translations)
            ));
            continue;
        }
        let name = path
            .strip_prefix(section)
            .unwrap_or(path)
            .trim_start_matches('.');
        let english = documented(path).expect("the enumeration documents every field");
        let meaning = one_line(
            translations
                .and_then(|tables| tables.field(path))
                .unwrap_or(english),
        );
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
            "\nRead selected exports: `tola help \"@tola/{name}\"{example}`\n",
        ));
    }
    if let Some(section) = package_config_table(name) {
        let header = table_header(section);
        text.push_str(&format!(
            "\n## Related\n\n- {}\n",
            named(&header, table_target(section), cross_refs)
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
        let parameters = match &export.declaration {
            ExportDeclaration::Function(signature) => {
                parameter_sections(translations, &export.name, signature)
            }
            ExportDeclaration::Value(_) => String::new(),
        };
        let documentation = &export.documentation;
        let localized = translations
            .and_then(|translations| translations.summary_prose(&export.name))
            .map(|prose| crate::i18n::localize_summary(&export.documentation.summary, prose));
        let summary = localized.as_deref().unwrap_or(&documentation.summary);
        text.push_str(&parameters);
        if !summary.is_empty() {
            if !parameters.is_empty() {
                // A heading names the export's own documentation, so no reader takes it for
                // another parameter.
                text.push_str("\n### Description\n");
            }
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
        let selection = PageId::PackageSelection {
            name: target.trim_start_matches("@tola/").to_owned(),
            exports: exports.iter().map(|name| (*name).to_owned()).collect(),
        };
        let named_exports = exports
            .iter()
            .map(|name| cross_reference(name, &LinkTarget::Page(selection.clone()), cross_refs))
            .collect::<Vec<_>>()
            .join(" ");
        text.push_str(&format!("- {named_exports}\n"));
    }
    for package in packages {
        let name = cross_reference(
            &format!("@tola/{package}"),
            &LinkTarget::Page(PageId::Package {
                name: package.to_owned(),
            }),
            cross_refs,
        );
        text.push_str(&format!("- {name}\n"));
    }
    text
}

/// The configuration table one bundled package configures, when a site author reads both together.
fn package_config_table(name: &str) -> Option<&'static str> {
    match name {
        "site" => Some("site"),
        "icon" => Some("icons"),
        _ => None,
    }
}

/// The header one configuration table renders under.
fn table_header(section: &str) -> String {
    TABLES
        .iter()
        .find(|table| table.section == section)
        .map(ConfigTable::header)
        .unwrap_or_else(|| format!("[{section}]"))
}

/// The bundled packages that configure one configuration table.
fn related_packages(section: &str) -> Vec<String> {
    tola_packages::builtin_packages()
        .map(|package| package.spec().name.to_string())
        .filter(|name| package_config_table(name) == Some(section))
        .collect()
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
        let mut requests = vec![vec![], targets(&["config"]), targets(&["package"])];
        requests.extend(
            TABLES
                .iter()
                .map(|table| vec![format!("[{}]", table.section)]),
        );
        for package in tola_packages::builtin_packages() {
            let name = package.spec().name.to_string();
            requests.push(vec![format!("@tola/{name}")]);
            let mut selected = vec![format!("@tola/{name}")];
            selected.extend(package.exports().take(2).map(str::to_owned));
            requests.push(selected);
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
            &["[typst]".to_owned()],
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
    fn config_index_lists_grouping_containers() {
        let index = page(
            &targets(&["config"]),
            HelpLanguage::English,
            CrossRefs::Suppress,
        )
        .unwrap();
        let markdown = index.markdown();
        assert!(markdown.contains("- `[build.hooks]`"), "{markdown}");
        assert!(!markdown.contains("- `[typst]`"), "{markdown}");
    }

    /// One relation shows on both sides: a package page heads its related section with the
    /// configuration table it serves, and that table names the package.
    #[test]
    fn related_targets_appear_on_both_sides() {
        let rendered = |arguments: &[&str]| {
            page(
                &targets(arguments),
                HelpLanguage::English,
                CrossRefs::Suppress,
            )
            .expect("the page builds")
            .markdown()
        };
        for package in tola_packages::builtin_packages() {
            let name = package.spec().name.to_string();
            let Some(section) = package_config_table(&name) else {
                continue;
            };
            let header = table_header(section);
            assert!(
                rendered(&[&format!("@tola/{name}")])
                    .contains(&format!("\n## Related\n\n- `{header}`\n")),
                "{name} does not head its related section with `{header}`"
            );
            assert!(
                rendered(&[&format!("[{section}]")]).contains(&format!("@tola/{name}")),
                "{section} does not name `@tola/{name}`"
            );
        }
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
        let selected = vec![
            "@tola/address".to_owned(),
            "url-of".into(),
            "slugify".into(),
        ];
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
        assert_eq!(names, ["url-of", "slugify"]);
    }

    #[test]
    fn unknown_exports_prevent_output() {
        let error = page(
            &[
                "@tola/address".to_owned(),
                "slugify".into(),
                "unknown".into(),
            ],
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
}
