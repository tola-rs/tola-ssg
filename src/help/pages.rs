//! Read-only help pages assembled from configuration and packages.

use anyhow::Result;

use tola_build::config::section::{
    AssetsConfig, FontsConfig, FormatterConfig, IconsConfig, SiteSectionConfig, TypstSectionConfig,
    VendorConfig,
    build::{
        BuildSectionConfig, HooksConfig, MinifyConfig, ReferencesConfig,
        hooks::{AfterPublishHookConfig, BeforeBuildHookConfig, OutputCommandConfig},
    },
};
use tola_diagnostic::{Diagnostic, DiagnosticCode, DiagnosticError, Severity};
use tola_packages::{ExportDeclaration, RelatedTarget, Signature};

use super::data::{Export, Key, Method, Parameter, ReferenceLink, Signature as ReferenceSignature};
use super::documentation::Documentation;
use crate::config::{DevConfig, DiagnosticsConfig, ServerConfig};
use crate::help::model::{Anchor, HelpCategory, HelpPage, LinkTarget, PageId, anchor};
use crate::i18n::{self, CONFIG_KEYS, CONFIG_USAGE, HelpLanguage, Translation};

struct ConfigTable {
    section: &'static str,
    array: bool,
}

impl ConfigTable {
    /// One table declared as `[section]`.
    const fn new(section: &'static str) -> Self {
        Self {
            section,
            array: false,
        }
    }

    /// One table declared as `[[section]]`.
    const fn new_array(section: &'static str) -> Self {
        Self {
            section,
            array: true,
        }
    }

    fn header(&self) -> String {
        if self.array {
            format!("[[{}]]", self.section)
        } else {
            format!("[{}]", self.section)
        }
    }
}

const TABLES: &[ConfigTable] = &[
    ConfigTable::new(SiteSectionConfig::TEMPLATE_SECTION),
    ConfigTable::new(BuildSectionConfig::TEMPLATE_SECTION),
    ConfigTable::new(MinifyConfig::TEMPLATE_SECTION),
    ConfigTable::new(ReferencesConfig::TEMPLATE_SECTION),
    ConfigTable::new(HooksConfig::TEMPLATE_SECTION),
    ConfigTable::new_array(BeforeBuildHookConfig::TEMPLATE_SECTION),
    ConfigTable::new_array(OutputCommandConfig::TEMPLATE_SECTION),
    ConfigTable::new_array(AfterPublishHookConfig::TEMPLATE_SECTION),
    ConfigTable::new(AssetsConfig::TEMPLATE_SECTION),
    ConfigTable::new(TypstSectionConfig::TEMPLATE_SECTION),
    ConfigTable::new(FontsConfig::TEMPLATE_SECTION),
    ConfigTable::new(IconsConfig::TEMPLATE_SECTION),
    ConfigTable::new("modules"),
    ConfigTable::new("options"),
    ConfigTable::new(VendorConfig::TEMPLATE_SECTION),
    ConfigTable::new(FormatterConfig::TEMPLATE_SECTION),
    ConfigTable::new(ServerConfig::TEMPLATE_SECTION),
    ConfigTable::new(DevConfig::TEMPLATE_SECTION),
    ConfigTable::new(DiagnosticsConfig::TEMPLATE_SECTION),
];

pub(crate) fn request(targets: &[String]) -> Result<LinkTarget> {
    super::target::request(targets).ok_or_else(|| {
        target_error(
            &targets.join(" "),
            "Use `tola help config`, `tola help package`, or `tola help scripts`",
        )
    })
}

pub(crate) fn page_of(id: &PageId, language: HelpLanguage) -> Result<HelpPage> {
    match id {
        PageId::Overview => Ok(overview_page(language)),
        PageId::Index(category) => Ok(index_page(*category, language)),
        PageId::Config { section } => table_page(section, language),
        PageId::Package { name } => package_page(name, &[], language),
        PageId::PackageSelection { name, exports } => package_page(name, exports, language),
        PageId::Script { name } => super::scripts::page(id.clone(), name, language),
    }
}

/// Link only the token so surrounding command quotes remain copyable.
fn cross_reference(token: &str, target: &LinkTarget) -> String {
    format!("[`{token}`]({})", target.uri())
}

/// One named token, linked to the page it names when the index can resolve it; a mention the
/// index cannot resolve stays plain text, because the index outlives a rename.
fn named(token: &str, target: Option<LinkTarget>) -> String {
    match target {
        Some(target) => cross_reference(token, &target),
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
    anchor(&format!("{} - {}", export.name, export_kind(export)))
}

pub(crate) fn render(markdown: &str, columns: Option<usize>, color: bool) -> String {
    let mut text = Documentation::new(columns, color).render(markdown);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn overview_page(language: HelpLanguage) -> HelpPage {
    let id = PageId::Overview;
    let mut markdown = format!("# Tola help\n\n{}\n", i18n::overview(language));
    for (title, category) in [
        ("Configuration", HelpCategory::Config),
        ("Packages", HelpCategory::Packages),
    ] {
        let linked = cross_reference(title, &LinkTarget::Page(PageId::Index(category)));
        markdown.push_str(&format!("\n- {linked}\n"));
    }
    markdown.push_str("\n## Examples\n\n");
    for (name, exports) in [
        ("source", &["tola-meta", "all-sources", "parse-sources"][..]),
        (
            "address",
            &["output-of", "output-url", "route-url", "asset-url"][..],
        ),
        ("document", &["headings", "references"][..]),
        ("diagnostic", &["report"][..]),
        ("head", &["head-metadata", "canonical"][..]),
        ("feed", &["feed"][..]),
        ("sitemap", &["sitemap"][..]),
    ] {
        let command = format!("tola help \"@tola/{name}\" {}", exports.join(" "));
        markdown.push_str(&format!(
            "- {}\n",
            named(&command, package_target(name, exports))
        ));
    }
    markdown.push_str(&format!(
        "- {}\n",
        named("tola help \"[build]\"", table_target("build"))
    ));
    markdown.push_str(&format!(
        "- {}\n",
        named("tola help \"[hooks]\"", table_target("hooks"))
    ));
    HelpPage::new(id, markdown)
}

fn index_page(category: HelpCategory, language: HelpLanguage) -> HelpPage {
    let id = PageId::Index(category);
    let mut markdown = String::new();
    let mut entries = Vec::new();
    match category {
        HelpCategory::Config => {
            markdown.push_str(match language {
                HelpLanguage::English => "# Configuration tables\n\nLook up settings in `tola.toml`. Use `tola help \"[build]\"` to find the publish directory, or `tola help \"[hooks]\"` to see when build scripts run.\n\n",
                HelpLanguage::SimplifiedChinese => "# 配置表\n\n查阅 `tola.toml`。用 `tola help --lang zh \"[build]\"` 查发布目录，用 `tola help --lang zh \"[hooks]\"` 查脚本在哪个构建阶段运行。\n\n",
            });
            // A table with settings of its own is an entry; so is a container grouping several
            // child tables.
            for table in TABLES.iter().filter(|table| {
                let children = child_tables(table.section).count();
                has_own_key(table.section)
                    || children > 1
                    || (children == 0 && section_help(table.section).is_some())
            }) {
                let title = table.header();
                let target =
                    table_target(table.section).expect("the index lists registered tables");
                let label = cross_reference(&title, &target);
                entries.push(ReferenceLink { title, target });
                markdown.push_str(&format!("- {label}\n"));
            }
        }
        HelpCategory::Packages => {
            markdown.push_str(match language {
                HelpLanguage::English => "# Bundled Typst packages\n\nLook up imports, parameters, and examples for `@tola/*` packages. To highlight code in HTML, start with `tola help \"@tola/code\" render-code`.\n\n",
                HelpLanguage::SimplifiedChinese => "# 内置 Typst 包\n\n查阅 `@tola/*` 包的导入方式、参数和示例。要给 HTML 中的代码加语法高亮，可以查 `tola help --lang zh \"@tola/code\" render-code`。\n\n",
            });
            for package in tola_packages::builtin_packages() {
                let name = package.spec().name.to_string();
                let title = format!("@tola/{name}");
                let target = LinkTarget::Page(PageId::Package { name });
                let label = cross_reference(&title, &target);
                entries.push(ReferenceLink { title, target });
                markdown.push_str(&format!("- {label}\n"));
            }
        }
        HelpCategory::Scripts => {
            markdown.push_str(match language {
                HelpLanguage::English => {
                    "# Built-in scripts\n\nRun these scripts with `tola nu`.\n\n"
                }
                HelpLanguage::SimplifiedChinese => "# 内置脚本\n\n用 `tola nu` 运行以下脚本。\n\n",
            });
            for script in super::scripts::builtins() {
                let title = format!("@tola-scripts/{}", script.name);
                let target = LinkTarget::Page(PageId::Script {
                    name: script.name.to_owned(),
                });
                let label = cross_reference(&title, &target);
                entries.push(ReferenceLink { title, target });
                let intro = i18n::script(language, script.name)
                    .and_then(Translation::intro)
                    .unwrap_or(script.summary);
                markdown.push_str(&format!("- {label}{}{intro}\n", language.colon()));
            }
        }
    }
    let mut page = HelpPage::new(id, markdown);
    page.reference.entries = entries;
    page
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

fn table_page(target: &str, language: HelpLanguage) -> Result<HelpPage> {
    let table = TABLES
        .iter()
        .find(|table| table.section == target)
        .ok_or_else(|| target_error(target, &table_choices()))?;
    // A container whose only content is its one child shows that child's page instead, so
    // the reader lands on the table that names real settings.
    if let Some(child) = sole_child_table(table.section) {
        return table_page(child.section, language);
    }
    let id = PageId::Config {
        section: table.section.to_owned(),
    };
    let mut markdown = format!("# {}\n", table.header());
    let translation = i18n::config(language, table.section);
    let intro = documented(table.section)
        .map(|english| translation.and_then(Translation::intro).unwrap_or(english));
    if let Some(documentation) = intro {
        markdown.push_str(&format!("\n{documentation}\n"));
    }
    if has_own_key(table.section) {
        markdown.push_str(&declared_keys(table.section, language));
    } else if child_tables(table.section).next().is_some() {
        markdown.push_str(match language {
            HelpLanguage::English => "\n**Tables**\n\n",
            HelpLanguage::SimplifiedChinese => "\n**子表**\n\n",
        });
        for child in child_tables(table.section) {
            let child_id = PageId::Config {
                section: child.section.to_owned(),
            };
            let header = child.header();
            // A table header is an array when its declaration is, so it includes its own brackets.
            let table = cross_reference(&header, &LinkTarget::Page(child_id));
            markdown.push_str(&format!(
                "- {table}{}{}\n",
                language.colon(),
                child_description(child, language)
            ));
        }
    }
    let usage = section_help(table.section).map(|english| {
        translation
            .and_then(|translation| translation.section(CONFIG_USAGE))
            .unwrap_or(english)
    });
    if let Some(help) = usage {
        markdown.push_str(&format!("\n{help}\n"));
    }
    let packages = related_packages(table.section);
    if !packages.is_empty() {
        markdown.push_str(&format!("\n### {}\n\n", language.related()));
        for package in &packages {
            markdown.push_str(&package_bullet(package));
        }
    }
    let mut page = HelpPage::new(id, markdown);
    page.reference.title = table.header();
    page.reference.blocks = page.prose(
        &[intro, usage]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    page.reference.entries = child_tables(table.section)
        .map(|child| ReferenceLink {
            title: child.header(),
            target: table_target(child.section).expect("child tables are registered"),
        })
        .collect();
    page.reference.keys = documented_fields(table.section)
        .into_iter()
        .filter(|path| child_table(path).is_none())
        .map(|path| Key {
            path: path.to_owned(),
            blocks: page.prose(key_prose(path, translation)),
        })
        .collect();
    page.reference.related = packages
        .into_iter()
        .map(|name| ReferenceLink {
            title: format!("@tola/{name}"),
            target: LinkTarget::Page(PageId::Package { name }),
        })
        .collect();
    Ok(page)
}

/// Every key this section's declaration writes, in schema order, without the nested ones: its
/// settings, and the keys opening child tables, each rendered as one bullet.
///
/// The renderer and the translation guard share this one enumeration.
fn documented_fields(section: &str) -> Vec<&'static str> {
    let prefix = format!("{section}.");
    tola_build::config::field_paths()
        .map(|path| path.as_str())
        .chain(crate::config::field_paths().map(|path| path.as_str()))
        .filter_map(|path| {
            let name = path.strip_prefix(&prefix)?;
            if name.contains('.') {
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
fn child_description(child: &ConfigTable, language: HelpLanguage) -> String {
    let english = documented(child.section).expect("every table documents itself");
    one_line(
        i18n::config(language, child.section)
            .and_then(Translation::intro)
            .unwrap_or(english),
    )
}

/// Whether a table holds nothing but one child table: no key of its own, and exactly one key
/// opening a table, so that table's page is the whole content the container has.
fn is_empty_container(section: &str) -> bool {
    let mut children = child_tables(section);
    children.next().is_some() && children.next().is_none() && !has_own_key(section)
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

/// Describe each setting or link to its child table. Keep examples inside their list item.
fn declared_keys(section: &str, language: HelpLanguage) -> String {
    let translation = i18n::config(language, section);
    let mut markdown = String::new();
    for path in documented_fields(section) {
        if let Some(child) = child_table(path) {
            let header = cross_reference(
                &child.header(),
                &LinkTarget::Page(PageId::Config {
                    section: child.section.to_owned(),
                }),
            );
            markdown.push_str(&format!(
                "\n- {header}{}{}",
                language.colon(),
                child_description(child, language)
            ));
            continue;
        }
        let name = path.rsplit_once('.').map_or(path, |(_, name)| name);
        markdown.push_str(&format!("\n- `{name}`"));
        append_description(&mut markdown, key_prose(path, translation), language);
    }
    markdown.push('\n');
    markdown
}

fn key_prose<'a>(path: &str, translation: Option<&'a Translation>) -> &'a str {
    let name = path.rsplit_once('.').map_or(path, |(_, name)| name);
    translation
        .and_then(|translation| translation.field(CONFIG_KEYS, name))
        .unwrap_or_else(|| documented(path).expect("the enumeration documents every field"))
}

/// One documentation paragraph whose lines the rendered page wraps itself.
fn one_line(documentation: &str) -> String {
    documentation
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn package_page(name: &str, requested: &[String], language: HelpLanguage) -> Result<HelpPage> {
    let package = tola_packages::builtin_packages()
        .find(|package| package.spec().name == name)
        .ok_or_else(|| target_error(name, &package_choices()))?;
    let target = format!("@tola/{name}");
    for member in requested {
        if !package.exports().any(|name| name == member) {
            return Err(help_error(
                crate::codes::help::MEMBER,
                format!("`{member}` is not an export of `{target}`"),
                choice_line("Exports", package.exports().map(quoted)),
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
    let translation = i18n::package(language, name);
    let import = format!("#import \"{}\": {imports}", package.spec());
    let import_heading = match language {
        HelpLanguage::English => "Import in Typst:",
        HelpLanguage::SimplifiedChinese => "导入",
    };
    let mut text = format!(
        "# {target}\n\n## {import_heading}\n\n{}",
        code_block("typst", &import)
    );
    let overview = whole_package.then(|| package.overview()).flatten();
    let overview = overview
        .as_deref()
        .map(|english| translation.and_then(Translation::intro).unwrap_or(english));
    if let Some(overview) = overview {
        text.push_str(&format!("\n{overview}\n"));
    }
    let exports = package.export_documentation(&names)?;
    if whole_package {
        let heading = match language {
            HelpLanguage::English => "Exports:",
            HelpLanguage::SimplifiedChinese => "导出成员",
        };
        text.push_str(&format!("\n## {heading}\n\n"));
        for export in &exports {
            let summary = translation
                .and_then(|translation| translation.section(&export.name))
                .map(summary_lead)
                .or_else(|| export_summary(export));
            let name = named(
                &export.name,
                Some(LinkTarget::PageAnchor(id.clone(), export_anchor(export))),
            );
            match summary {
                Some(summary) => {
                    text.push_str(&format!("- {name}{}{summary}\n", language.colon()));
                }
                None => text.push_str(&format!("- {name}\n")),
            }
        }
    }
    let config = package_config_table(name);
    if let Some(section) = config {
        let header = table_header(section);
        text.push_str(&format!(
            "\n## {}\n\n- {}\n",
            language.related(),
            named(&header, table_target(section))
        ));
    }
    let mut page = HelpPage::new(id.clone(), text);
    page.reference.title = target.clone();
    page.reference.blocks = page.prose(overview.unwrap_or_default());
    page.reference.import = Some(import);
    page.reference.related = config
        .into_iter()
        .filter_map(|section| {
            Some(ReferenceLink {
                title: table_header(section),
                target: table_target(section)?,
            })
        })
        .collect();
    let canonical = PageId::Package {
        name: name.to_owned(),
    };
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
        let summary = translation
            .and_then(|translation| translation.section(&export.name))
            .unwrap_or(&export.documentation.summary);
        if !summary.is_empty() {
            text.push_str(&format!("\n{summary}\n"));
        }
        let parameter_prose =
            |name: &str| translation.and_then(|translation| translation.field(&export.name, name));
        if let ExportDeclaration::Function(signature) = &export.declaration {
            write_parameters(&mut text, signature, language, parameter_prose);
        }
        let mut methods = Vec::new();
        for method in &export.methods {
            let receiver = export
                .name
                .chars()
                .next()
                .expect("export names are nonempty");
            let name = &export.name;
            let receiver_help = match language {
                HelpLanguage::English => {
                    format!("`{receiver}` is the value returned by `{name}(...)`.")
                }
                HelpLanguage::SimplifiedChinese => {
                    format!("`{receiver}` 是 `{name}(...)` 的返回值。")
                }
            };
            let parameters = translation
                .map(|translation| translation.method_parameters(&export.name, &method.name))
                .unwrap_or_default();
            let prose = |name: &str| {
                parameters
                    .get(name)
                    .copied()
                    .filter(|text| !text.is_empty())
            };
            let declaration = method.method_code(receiver);
            text.push_str(&format!(
                "\n### `{}`\n\n{}\n{receiver_help}\n",
                method.name,
                code_block("typst-code", &declaration),
            ));
            write_parameters(&mut text, method, language, prose);
            methods.push(Method {
                name: method.name.clone(),
                anchor: anchor(&method.name).as_str().to_owned(),
                signature: reference_signature(&page, method, declaration, prose),
                blocks: page.prose(&receiver_help),
            });
        }
        text.push_str(&related_section(&target, &export.related, &names, language));
        page.push_export(text);
        let signature = match &export.declaration {
            ExportDeclaration::Function(signature) => {
                reference_signature(&page, signature, signature.code(), parameter_prose)
            }
            ExportDeclaration::Value(value) => ReferenceSignature {
                declaration: value.clone(),
                parameters: Vec::new(),
                returns: None,
            },
        };
        let mut related = Vec::new();
        for item in &export.related {
            let link = match item {
                RelatedTarget::Export(name) if name != &export.name => {
                    let other =
                        if let Some(other) = exports.iter().find(|export| &export.name == name) {
                            export_anchor(other)
                        } else {
                            let other = package.export_documentation(std::slice::from_ref(name))?;
                            export_anchor(&other[0])
                        };
                    ReferenceLink {
                        title: name.clone(),
                        target: LinkTarget::PageAnchor(canonical.clone(), other),
                    }
                }
                RelatedTarget::Package(package) if package != name => ReferenceLink {
                    title: format!("@tola/{package}"),
                    target: LinkTarget::Page(PageId::Package {
                        name: package.clone(),
                    }),
                },
                _ => continue,
            };
            if !related
                .iter()
                .any(|item: &ReferenceLink| item.target == link.target)
            {
                related.push(link);
            }
        }
        page.reference.exports.push(Export {
            name: export.name.clone(),
            kind: export_kind(export),
            anchor: export_anchor(export).as_str().to_owned(),
            signature,
            blocks: page.prose(summary),
            methods,
            related,
        });
    }
    Ok(page)
}

fn reference_signature<'a>(
    page: &HelpPage,
    signature: &Signature,
    declaration: String,
    prose: impl Fn(&str) -> Option<&'a str>,
) -> ReferenceSignature {
    use tola_packages::ParameterKind::{Named, Positional, Rest};
    ReferenceSignature {
        declaration,
        parameters: signature
            .parameters
            .iter()
            .map(|parameter| Parameter {
                name: parameter.name.clone(),
                kind: match parameter.kind {
                    Positional => "positional",
                    Named => "named",
                    Rest => "rest",
                },
                ty: Some(parameter.ty_spelling().to_owned()),
                required: parameter.required,
                default: parameter.default.clone(),
                blocks: page.prose(
                    prose(&parameter.name)
                        .or(parameter.docs.as_deref())
                        .unwrap_or_default(),
                ),
                short: None,
            })
            .collect(),
        returns: signature.returns.clone(),
    }
}

/// The declaration block every export is configured with: a function's signature, or a value's
/// own line.
fn declaration(declaration: &ExportDeclaration) -> String {
    match declaration {
        ExportDeclaration::Function(signature) => signature.code(),
        ExportDeclaration::Value(value) => value.clone(),
    }
}

/// Describe parameters by name. Their types and defaults appear in the signature.
fn write_parameters<'a>(
    text: &mut String,
    signature: &Signature,
    language: HelpLanguage,
    prose: impl Fn(&str) -> Option<&'a str>,
) {
    let (positional, rest, named, required) = match language {
        HelpLanguage::English => (
            "Positional parameters",
            "Rest parameters",
            "Named parameters",
            "required",
        ),
        HelpLanguage::SimplifiedChinese => ("位置参数", "可变参数", "命名参数", "必填"),
    };
    let (open, close) = language.parentheses();
    for (heading, group) in [
        (positional, signature.positional()),
        (rest, signature.rest()),
        (named, signature.named()),
    ] {
        if group.is_empty() {
            continue;
        }
        text.push_str(&format!("\n**{heading}**\n\n"));
        for parameter in group {
            let prefix = if parameter.kind == tola_packages::ParameterKind::Rest {
                ".."
            } else {
                ""
            };
            text.push_str(&format!("- `{prefix}{}`", parameter.name));
            if parameter.required {
                text.push_str(&format!("{open}{required}{close}"));
            }
            let documentation = prose(&parameter.name).or(parameter.docs.as_deref());
            if let Some(documentation) = documentation {
                append_description(text, documentation, language);
            }
            text.push('\n');
        }
    }
}

/// Finish a list item with its Markdown description, including any nested blocks.
fn append_description(text: &mut String, documentation: &str, language: HelpLanguage) {
    let mut lines = documentation.lines();
    if let Some(first) = lines.next() {
        text.push_str(language.colon());
        text.push_str(first);
    }
    for line in lines {
        text.push('\n');
        if !line.is_empty() {
            text.push_str("  ");
            text.push_str(line);
        }
    }
}

/// The related-targets section one export renders: the package's own exports as one command,
/// then each bundled package as its own, in the order the block first names them. A target the
/// page renders anyway is left out, and a block whose targets are all rendered has no section.
fn related_section(
    target: &str,
    related: &[RelatedTarget],
    rendered: &[String],
    language: HelpLanguage,
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
    let mut text = format!("\n### {}\n\n", language.related());
    if !exports.is_empty() {
        let selection = PageId::PackageSelection {
            name: target.trim_start_matches("@tola/").to_owned(),
            exports: exports.iter().map(|name| (*name).to_owned()).collect(),
        };
        let named_exports = exports
            .iter()
            .map(|name| cross_reference(name, &LinkTarget::Page(selection.clone())))
            .collect::<Vec<_>>()
            .join(" ");
        text.push_str(&format!("- {named_exports}\n"));
    }
    for package in packages {
        text.push_str(&package_bullet(package));
    }
    text
}

/// One bullet naming a bundled package's page by its Typst import.
fn package_bullet(package: &str) -> String {
    let name = cross_reference(
        &format!("@tola/{package}"),
        &LinkTarget::Page(PageId::Package {
            name: package.to_owned(),
        }),
    );
    format!("- {name}\n")
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

pub(super) fn code_block(language: &str, source: &str) -> String {
    let longest = source
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let newline = if source.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{language}\n{source}{newline}{fence}\n")
}

/// One help line naming every selector a reader can pass instead.
fn choice_line(label: &str, choices: impl IntoIterator<Item = String>) -> String {
    format!(
        "{label}: {}",
        choices.into_iter().collect::<Vec<_>>().join(", ")
    )
}

fn table_choices() -> String {
    choice_line("Configuration tables", config_sections())
}

fn package_choices() -> String {
    choice_line("Bundled packages", package_names())
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

pub(super) fn target_error(target: &str, choices: &str) -> anyhow::Error {
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

    fn contains_anchor(blocks: &[Block], wanted: &Anchor) -> bool {
        blocks.iter().any(|block| match block {
            Block::Heading { anchor, .. } => anchor == wanted,
            Block::List { items, .. } => items.iter().any(|blocks| contains_anchor(blocks, wanted)),
            Block::Quote { blocks } => contains_anchor(blocks, wanted),
            _ => false,
        })
    }

    fn page(targets: &[String], language: HelpLanguage) -> Result<HelpPage> {
        let target = request(targets)?;
        page_of(
            target.page().expect("a help request locates a page"),
            language,
        )
    }

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
            targets(&["scripts"]),
        ];
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
    fn config_translations_name_existing_tables_and_keys() {
        for section in i18n::config_names() {
            assert!(
                TABLES.iter().any(|table| table.section == section),
                "`{section}` is not a configuration table"
            );
            let translation = i18n::config(HelpLanguage::SimplifiedChinese, section).unwrap();
            for heading in translation.sections() {
                assert!(
                    matches!(heading, CONFIG_KEYS | CONFIG_USAGE),
                    "`{section}` has an unknown translation heading `{heading}`"
                );
            }
            let fields = documented_fields(section);
            for name in translation.fields(CONFIG_KEYS) {
                let path = format!("{section}.{name}");
                assert!(
                    fields.contains(&path.as_str()) && child_table(&path).is_none(),
                    "`{path}` is not a key this table documents"
                );
            }
        }
    }

    #[test]
    fn empty_container_shows_child_settings() {
        let page = page(&["[typst]".to_owned()], HelpLanguage::English).unwrap();
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
        let index =
            HelpDocument::parse(&page(&targets(&["config"]), HelpLanguage::English).unwrap());
        let links = cross_references(&index);
        for (section, shown) in [
            ("hooks", true),
            ("modules", true),
            ("options", true),
            ("typst", false),
        ] {
            let target = PageId::Config {
                section: section.into(),
            };
            assert_eq!(
                links.iter().any(|link| link.page() == Some(&target)),
                shown,
                "{section}"
            );
        }
    }

    #[test]
    fn related_targets_appear_on_both_sides() {
        let related = |arguments: &[&str]| {
            let document =
                HelpDocument::parse(&page(&targets(arguments), HelpLanguage::English).unwrap());
            cross_references(&document)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        };
        for package in tola_packages::builtin_packages() {
            let name = package.spec().name.to_string();
            let Some(section) = package_config_table(&name) else {
                continue;
            };
            assert!(
                related(&[&format!("@tola/{name}")]).contains(&LinkTarget::Page(PageId::Config {
                    section: section.into()
                })),
                "{name} does not link to {section}"
            );
            assert!(
                related(&[&format!("[{section}]")])
                    .contains(&LinkTarget::Page(PageId::Package { name: name.clone() })),
                "{section} does not link to {name}"
            );
        }
    }

    #[test]
    fn help_links_resolve_registered_pages() {
        let mut loaded: Vec<(PageId, HelpDocument)> = Vec::new();
        for arguments in requests() {
            let document = HelpDocument::parse(&page(&arguments, HelpLanguage::English).unwrap());
            for target in cross_references(&document) {
                match target {
                    LinkTarget::External(uri) => {
                        assert!(!uri.starts_with(CROSS_REFERENCE_SCHEME), "{uri}")
                    }
                    LinkTarget::Page(id) | LinkTarget::PageAnchor(id, _) => {
                        let document = if let Some((_, document)) =
                            loaded.iter().find(|(page, _)| page == id)
                        {
                            document.clone()
                        } else {
                            let document =
                                HelpDocument::parse(&page_of(id, HelpLanguage::English).unwrap());
                            loaded.push((id.clone(), document.clone()));
                            document
                        };
                        if let LinkTarget::PageAnchor(_, anchor) = target {
                            assert!(
                                contains_anchor(&document.blocks, anchor),
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

    fn cross_references(document: &HelpDocument) -> Vec<&LinkTarget> {
        let mut found = Vec::new();
        for block in &document.blocks {
            walk_block(block, &mut found);
        }
        found
    }

    fn walk_block<'a>(block: &'a Block, found: &mut Vec<&'a LinkTarget>) {
        match block {
            Block::Paragraph { spans, .. } | Block::Heading { spans, .. } => {
                walk_spans(spans, found)
            }
            Block::List { items, .. } => {
                for item in items {
                    for block in item {
                        walk_block(block, found);
                    }
                }
            }
            Block::Quote { blocks } => {
                for block in blocks {
                    walk_block(block, found);
                }
            }
            Block::Table { headers, rows, .. } => {
                for spans in headers.iter().chain(rows.iter().flatten()) {
                    walk_spans(spans, found);
                }
            }
            Block::Code { .. } | Block::Rule => {}
        }
    }

    fn walk_spans<'a>(spans: &'a [Inline], found: &mut Vec<&'a LinkTarget>) {
        for span in spans {
            match span {
                Inline::Link { label, target } => {
                    found.push(target);
                    walk_spans(label, found);
                }
                Inline::Emph(children) | Inline::Strong(children) => {
                    walk_spans(children, found);
                }
                Inline::Text(_) | Inline::Code(_) => {}
            }
        }
    }

    #[test]
    fn selected_exports_preserve_order() {
        let selected = vec![
            "@tola/address".to_owned(),
            "output-url".into(),
            "slugify".into(),
        ];
        let document = HelpDocument::parse(&page(&selected, HelpLanguage::English).unwrap());
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
        assert_eq!(names, ["output-url", "slugify"]);
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
        )
        .err()
        .expect("an unknown export is rejected");
        assert_eq!(
            tola_diagnostic::attached(&error).unwrap()[0].code,
            crate::codes::help::MEMBER
        );
    }
}
