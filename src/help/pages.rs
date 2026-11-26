//! Assemble typed `HelpPage` and `HelpCatalog` values from code schemas and `.typ` docs.

use std::sync::LazyLock;

use anyhow::{Result, bail};
use tola_build::config::SiteConfigSchema;
use tola_build::config::section::{
    AssetsConfig, FontsConfig, FormatterConfig, IconsConfig, SiteSectionConfig, TypstSectionConfig,
    VendorConfig,
    build::{
        BuildSectionConfig, HooksConfig, MinifyConfig, ReferencesConfig,
        hooks::{AfterPublishHookConfig, BeforeBuildHookConfig, OutputCommandConfig},
    },
};
use tola_diagnostic::{Diagnostic, DiagnosticCode, DiagnosticError, Severity};
use tola_packages::{ExportDeclaration, Signature};

use super::docs;
use super::language::HelpLanguage;
use super::markup::Markup;
use super::model::{
    CategoryEntry, ConfigChild, ConfigKey, ConfigPage, HelpCatalog, HelpCategory, HelpPage,
    IndexEntry, IndexPage, LinkTarget, OverviewPage, PackageExport, PackageMethod, PackagePage,
    PageId, Parameter, RelatedItem,
};
use crate::config::{DevConfig, DiagnosticsConfig, ServerConfig};

struct ConfigTable {
    section: &'static str,
    array: bool,
    key: bool,
}

impl ConfigTable {
    const fn new(section: &'static str) -> Self {
        Self {
            section,
            array: false,
            key: false,
        }
    }

    const fn new_array(section: &'static str) -> Self {
        Self {
            section,
            array: true,
            key: false,
        }
    }

    const fn new_key(section: &'static str) -> Self {
        Self {
            section,
            array: false,
            key: true,
        }
    }

    fn header(&self) -> String {
        if self.key {
            self.section.to_owned()
        } else if self.array {
            format!("[[{}]]", self.section)
        } else {
            format!("[{}]", self.section)
        }
    }
}

const TABLES: &[ConfigTable] = &[
    ConfigTable::new_key("include"),
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

static DEFAULT_CONFIG: LazyLock<serde_json::Value> = LazyLock::new(|| {
    let mut root = serde_json::to_value(SiteConfigSchema::default())
        .expect("SiteConfigSchema::default must serialize");
    if let serde_json::Value::Object(map) = &mut root {
        map.insert(
            ServerConfig::TEMPLATE_SECTION.to_owned(),
            serde_json::to_value(ServerConfig::default())
                .expect("ServerConfig::default must serialize"),
        );
        map.insert(
            DevConfig::TEMPLATE_SECTION.to_owned(),
            serde_json::to_value(DevConfig::default()).expect("DevConfig::default must serialize"),
        );
        map.insert(
            DiagnosticsConfig::TEMPLATE_SECTION.to_owned(),
            serde_json::to_value(DiagnosticsConfig::default())
                .expect("DiagnosticsConfig::default must serialize"),
        );
    }
    root
});

fn default_value(path: &str) -> Option<serde_json::Value> {
    let mut current = &*DEFAULT_CONFIG;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    if current.is_null() {
        return None;
    }
    Some(current.clone())
}

fn config_table(section: &str) -> Option<&'static ConfigTable> {
    TABLES.iter().find(|table| table.section == section)
}

pub(super) fn config_section(selector: &str) -> Option<&'static str> {
    let table = config_table(selector.trim_matches(['[', ']']))?;
    (selector == table.header()).then_some(table.section)
}

pub(crate) fn table_header(section: &str) -> String {
    config_table(section)
        .map(ConfigTable::header)
        .unwrap_or_else(|| format!("[{section}]"))
}

pub(crate) fn request(targets: &[String]) -> Result<LinkTarget> {
    super::target::request(targets).ok_or_else(|| {
        let help = if let [selector] = targets
            && selector.starts_with('[')
        {
            config_table(selector.trim_matches(['[', ']'])).map_or_else(table_choices, |table| {
                format!("Use `tola help {}`", quoted(&table.header()))
            })
        } else {
            "Use `tola help config`, `tola help package`, or `tola help scripts`".to_owned()
        };
        target_error(&targets.join(" "), &help)
    })
}

pub(crate) fn page_for_target(target: &LinkTarget, language: HelpLanguage) -> Result<HelpPage> {
    match target {
        LinkTarget::Page(id) => page_of(id, language),
        LinkTarget::PageAnchor(PageId::Script { name }, anchor) => Ok(HelpPage::Script(
            super::scripts::page(name, Some(anchor.as_str()), language)?,
        )),
        LinkTarget::PageAnchor(id, _) => page_of(id, language),
        LinkTarget::External(uri) => bail!("`{uri}` is an external URL, not a help page"),
    }
}

pub(crate) fn page_of(id: &PageId, language: HelpLanguage) -> Result<HelpPage> {
    match id {
        PageId::Overview => Ok(HelpPage::Overview(overview_page(language))),
        PageId::Index(category) => Ok(HelpPage::Index(index_page(*category, language))),
        PageId::Config { section } => Ok(HelpPage::Config(config_page(section, language)?)),
        PageId::Package { name } => Ok(HelpPage::Package(package_page(name, &[], language)?)),
        PageId::PackageSelection { name, exports } => {
            Ok(HelpPage::Package(package_page(name, exports, language)?))
        }
        PageId::Script { name } => Ok(HelpPage::Script(super::scripts::page(
            name, None, language,
        )?)),
    }
}

/// Every reference page for one language in canonical order (`tola help --all`).
pub(crate) fn catalog(language: HelpLanguage) -> Result<HelpCatalog> {
    let overview = overview_page(language);
    let mut config = Vec::new();
    for table in indexed_config_tables() {
        config.push(config_page(table.section, language)?);
    }
    let mut packages = Vec::new();
    for package in tola_packages::builtin_packages() {
        let spec = package.spec();
        packages.push(package_page(spec.name.as_str(), &[], language)?);
    }
    let mut scripts = Vec::new();
    for script in super::scripts::builtins() {
        scripts.push(super::scripts::page(script.name, None, language)?);
    }
    Ok(HelpCatalog {
        kind: "catalog",
        language: language.tag(),
        overview,
        config,
        packages,
        scripts,
    })
}

/// Tables listed in the config index and catalog (omits single-child transparent containers like `[typst]`).
fn indexed_config_tables() -> impl Iterator<Item = &'static ConfigTable> {
    TABLES
        .iter()
        .filter(|table| !is_empty_container(table.section))
}

fn overview_page(lang: HelpLanguage) -> OverviewPage {
    let doc = docs::overview(lang);
    let (title, cfg_t, pkg_t, scr_t) = match lang {
        HelpLanguage::English => ("Tola help", "Configuration", "Packages", "Scripts"),
        HelpLanguage::SimplifiedChinese => ("Tola 帮助", "配置", "内置包", "内置脚本"),
    };
    OverviewPage {
        language: lang.tag(),
        title: title.to_owned(),
        body: doc.body.clone(),
        categories: vec![
            CategoryEntry {
                category: "config",
                title: cfg_t,
            },
            CategoryEntry {
                category: "packages",
                title: pkg_t,
            },
            CategoryEntry {
                category: "scripts",
                title: scr_t,
            },
        ],
    }
}

fn build_index(
    cat: &'static str,
    lang: HelpLanguage,
    en: (&str, &str),
    zh: (&str, &str),
    entries: Vec<IndexEntry>,
) -> IndexPage {
    let (title, summary) = match lang {
        HelpLanguage::English => en,
        HelpLanguage::SimplifiedChinese => zh,
    };
    IndexPage {
        category: cat,
        language: lang.tag(),
        title: title.to_owned(),
        summary: Markup::parse(summary).expect("valid index summary"),
        entries,
    }
}

fn index_page(cat: HelpCategory, lang: HelpLanguage) -> IndexPage {
    match cat {
        HelpCategory::Config => {
            let entries = indexed_config_tables()
                .map(|t| IndexEntry {
                    id: t.section.to_owned(),
                    title: t.header(),
                    summary: docs::config(lang, t.section)
                        .map(|d| d.summary.clone())
                        .unwrap_or_default(),
                })
                .collect();
            build_index(
                "config",
                lang,
                (
                    "Configuration",
                    "Reference for `tola.toml`. Run `tola help \"[build]\"` for build paths or `tola help \"[hooks]\"` for build stages.",
                ),
                (
                    "配置",
                    "`tola.toml` 配置参考。运行 `tola help --lang zh \"[build]\"` 查看构建路径，或 `tola help --lang zh \"[hooks]\"` 查看构建阶段。",
                ),
                entries,
            )
        }
        HelpCategory::Packages => {
            let entries = tola_packages::builtin_packages()
                .map(|pkg| {
                    let name = pkg.spec().name.to_string();
                    IndexEntry {
                        summary: docs::package(lang, &name)
                            .map(|d| d.summary.clone())
                            .unwrap_or_default(),
                        title: format!("@tola/{name}"),
                        id: name,
                    }
                })
                .collect();
            build_index(
                "packages",
                lang,
                (
                    "Bundled Typst packages",
                    "Reference for built-in `@tola/*` packages. Run `tola help \"@tola/source\"` to inspect content discovery and metadata.",
                ),
                (
                    "内置 Typst 包",
                    "内置 `@tola/*` 包参考。运行 `tola help --lang zh \"@tola/source\"` 查看内容发现与元数据接口。",
                ),
                entries,
            )
        }
        HelpCategory::Scripts => {
            let entries = super::scripts::builtins()
                .map(|s| IndexEntry {
                    id: s.name.to_owned(),
                    title: format!("@tola-scripts/{}", s.name),
                    summary: docs::script(lang, s.name)
                        .map(|d| d.summary.clone())
                        .unwrap_or_default(),
                })
                .collect();
            build_index(
                "scripts",
                lang,
                (
                    "Built-in scripts",
                    "Utility scripts bundled with `tola nu`.",
                ),
                ("内置脚本", "随 `tola nu` 提供的内置脚本。"),
                entries,
            )
        }
    }
}

fn config_page(target: &str, lang: HelpLanguage) -> Result<ConfigPage> {
    let table = config_table(target).ok_or_else(|| target_error(target, &table_choices()))?;
    if let Some(child) = sole_child_table(table.section) {
        return config_page(child.section, lang);
    }
    let doc = docs::config(lang, table.section)
        .unwrap_or_else(|| panic!("missing config docs for `{}`", table.section));

    let keys = documented_fields(table.section)
        .into_iter()
        .filter(|path| child_table(path).is_none())
        .map(|path| {
            let name = path.rsplit_once('.').map_or(path, |(_, n)| n);
            let kdoc = doc.keys.get(name);
            let body = kdoc.map(|i| i.body.clone()).unwrap_or_else(|| {
                documented(path)
                    .and_then(|fb| Markup::parse(fb).ok())
                    .unwrap_or_default()
            });
            ConfigKey {
                name: name.to_owned(),
                path: path.to_owned(),
                default: default_value(path),
                values: tola_build::config::field_values(path).map(|s| s.to_vec()),
                body,
                examples: kdoc.map(|i| i.examples.clone()).unwrap_or_default(),
            }
        })
        .collect();

    let children = child_tables(table.section)
        .map(|child| ConfigChild {
            section: child.section.to_owned(),
            header: child.header(),
            summary: docs::config(lang, child.section)
                .map(|d| d.summary.clone())
                .unwrap_or_default(),
        })
        .collect();

    let mut related = resolve_related_list(&doc.related, None)?;
    for pkg_name in related_packages(table.section) {
        let item = RelatedItem::Package {
            title: format!("@tola/{pkg_name}"),
            name: pkg_name,
        };
        if !related.contains(&item) {
            related.push(item);
        }
    }

    Ok(ConfigPage {
        section: table.section.to_owned(),
        header: table.header(),
        language: lang.tag(),
        summary: doc.summary.clone(),
        body: doc.body.clone(),
        keys,
        children,
        examples: doc.examples.clone(),
        related,
    })
}

fn builtin_pkg(name: &str) -> Option<tola_packages::BuiltinPackage> {
    tola_packages::builtin_packages().find(|pkg| pkg.spec().name == name)
}

fn build_package_method(
    export_name: &str,
    method: &Signature,
    mdoc: Option<&super::markup::MethodDoc>,
    lang: HelpLanguage,
) -> PackageMethod {
    let recv = export_name
        .chars()
        .next()
        .expect("export names are nonempty");
    let recv_text = match lang {
        HelpLanguage::English => format!("`{recv}` is the value returned by `{export_name}(...)`."),
        HelpLanguage::SimplifiedChinese => format!("`{recv}` 是 `{export_name}(...)` 的返回值。"),
    };
    PackageMethod {
        name: method.name.clone(),
        declaration: method.method_code(recv),
        returns: method.returns.clone(),
        receiver_note: Markup::parse(&recv_text).expect("valid receiver note"),
        summary: mdoc.map(|m| m.summary.clone()).unwrap_or_default(),
        body: mdoc.map(|m| m.body.clone()).unwrap_or_default(),
        parameters: build_parameters(method, |p| mdoc.and_then(|m| m.params.get(p))),
        examples: mdoc.map(|m| m.examples.clone()).unwrap_or_default(),
    }
}

fn package_page(name: &str, req: &[String], lang: HelpLanguage) -> Result<PackagePage> {
    let pkg = builtin_pkg(name).ok_or_else(|| target_error(name, &package_choices()))?;
    let target = format!("@tola/{name}");
    for member in req {
        if !pkg.exports().any(|e| e == member) {
            return Err(help_error(
                crate::codes::help::MEMBER,
                format!("`{member}` is not an export of `{target}`"),
                choice_line("Exports", pkg.exports().map(quoted)),
            ));
        }
    }
    let whole = req.is_empty();
    let names = if whole {
        pkg.exports().map(str::to_owned).collect::<Vec<_>>()
    } else {
        req.to_vec()
    };
    let imports = if whole {
        "*".to_owned()
    } else {
        names.join(", ")
    };
    let import = format!("#import \"{}\": {imports}", pkg.spec());
    let doc =
        docs::package(lang, name).unwrap_or_else(|| panic!("missing package docs for `{name}`"));

    let mut exports = Vec::new();
    for exp in pkg.export_documentation(&names)? {
        let edoc = doc.exports.get(&exp.name);
        let (decl, returns, params) = match &exp.declaration {
            ExportDeclaration::Function(sig) => (
                sig.code(),
                sig.returns.clone(),
                build_parameters(sig, |p| edoc.and_then(|d| d.params.get(p))),
            ),
            ExportDeclaration::Value(val) => (val.clone(), None, Vec::new()),
        };
        let methods = exp
            .methods
            .iter()
            .map(|m| {
                build_package_method(
                    &exp.name,
                    m,
                    edoc.and_then(|d| d.methods.get(&m.name)),
                    lang,
                )
            })
            .collect();
        let related = edoc
            .map(|d| resolve_related_list(&d.related, Some(name)))
            .transpose()?
            .unwrap_or_default();
        exports.push(PackageExport {
            name: exp.name.clone(),
            kind: if exp.declaration.is_function() {
                "function"
            } else {
                "value"
            },
            declaration: decl,
            returns,
            summary: edoc.map(|d| d.summary.clone()).unwrap_or_default(),
            body: edoc.map(|d| d.body.clone()).unwrap_or_default(),
            parameters: params,
            methods,
            examples: edoc.map(|d| d.examples.clone()).unwrap_or_default(),
            related,
        });
    }

    let mut related = resolve_related_list(&doc.related, Some(name))?;
    if let Some(section) = package_config_table(name) {
        let item = RelatedItem::Config {
            section: section.to_owned(),
            header: table_header(section),
        };
        if !related.contains(&item) {
            related.push(item);
        }
    }

    Ok(PackagePage {
        name: name.to_owned(),
        spec: pkg.spec().to_string(),
        title: target,
        import,
        language: lang.tag(),
        summary: if whole {
            doc.summary.clone()
        } else {
            Markup::default()
        },
        body: if whole {
            doc.body.clone()
        } else {
            Markup::default()
        },
        exports,
        examples: if whole {
            doc.examples.clone()
        } else {
            Vec::new()
        },
        related: if whole { related } else { Vec::new() },
        selected: !whole,
    })
}

fn build_parameters<'a>(
    sig: &Signature,
    lookup: impl Fn(&str) -> Option<&'a Markup>,
) -> Vec<Parameter> {
    use tola_packages::ParameterKind::{Named, Positional, Rest};
    sig.parameters
        .iter()
        .map(|p| Parameter {
            name: p.name.clone(),
            kind: match p.kind {
                Positional => "positional",
                Named => "named",
                Rest => "rest",
            },
            ty: Some(p.ty_spelling().to_owned()),
            required: p.required,
            default: p.default.clone(),
            short: None,
            description: lookup(&p.name).cloned().unwrap_or_default(),
        })
        .collect()
}

/// Resolve a `#ref(...)` or `related` target string into a typed `RelatedItem`.
pub(crate) fn resolve_reference(raw: &str, cur_pkg: Option<&str>) -> Result<RelatedItem> {
    if let Some(section) = config_section(raw) {
        return Ok(RelatedItem::Config {
            section: section.to_owned(),
            header: table_header(section),
        });
    }
    if let Some(rest) = raw.strip_prefix("@tola-scripts/") {
        if let Some((script, cmd)) = rest.split_once('#') {
            if super::scripts::builtins().any(|s| s.name == script) {
                return Ok(RelatedItem::Command {
                    script: script.to_owned(),
                    name: cmd.to_owned(),
                });
            }
        } else if super::scripts::builtins().any(|s| s.name == rest) {
            return Ok(RelatedItem::Script {
                name: rest.to_owned(),
                locator: format!("@tola-scripts/{rest}"),
            });
        }
        bail!("unknown script reference `{raw}`");
    }
    if let Some(rest) = raw.strip_prefix("@tola/") {
        if let Some((pkg_name, exp_name)) = rest.split_once('#') {
            let pkg = builtin_pkg(pkg_name)
                .ok_or_else(|| anyhow::anyhow!("unknown package in reference `{raw}`"))?;
            if !pkg.exports().any(|e| e == exp_name) {
                bail!("unknown export `{exp_name}` in reference `{raw}`");
            }
            return Ok(RelatedItem::Export {
                package: pkg_name.to_owned(),
                name: exp_name.to_owned(),
            });
        }
        if builtin_pkg(rest).is_some() {
            return Ok(RelatedItem::Package {
                name: rest.to_owned(),
                title: format!("@tola/{rest}"),
            });
        }
        bail!("unknown package reference `{raw}`");
    }
    if let Some(pkg_name) = cur_pkg
        && let Some(pkg) = builtin_pkg(pkg_name)
        && pkg.exports().any(|e| e == raw)
    {
        return Ok(RelatedItem::Export {
            package: pkg_name.to_owned(),
            name: raw.to_owned(),
        });
    }
    bail!("unresolved help reference `{raw}`")
}

fn resolve_related_list(
    targets: &[String],
    current_package: Option<&str>,
) -> Result<Vec<RelatedItem>> {
    let mut items = Vec::new();
    for raw in targets {
        let item = resolve_reference(raw, current_package)?;
        if !items.contains(&item) {
            items.push(item);
        }
    }
    Ok(items)
}

pub(crate) fn documented_fields(section: &str) -> Vec<&'static str> {
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

fn documented(path: &str) -> Option<&'static str> {
    tola_build::config::field_documentation(path)
        .or_else(|| crate::config::field_documentation(path))
}

fn child_table(path: &str) -> Option<&'static ConfigTable> {
    TABLES.iter().find(|child| child.section == path)
}

fn child_tables(section: &str) -> impl Iterator<Item = &'static ConfigTable> {
    let prefix = format!("{section}.");
    TABLES.iter().filter(move |table| {
        table
            .section
            .strip_prefix(&prefix)
            .is_some_and(|suffix| !suffix.contains('.'))
    })
}

fn has_own_key(section: &str) -> bool {
    documented_fields(section)
        .iter()
        .any(|path| child_table(path).is_none())
}

fn is_empty_container(section: &str) -> bool {
    let mut children = child_tables(section);
    children.next().is_some() && children.next().is_none() && !has_own_key(section)
}

fn sole_child_table(section: &str) -> Option<&'static ConfigTable> {
    is_empty_container(section)
        .then(|| child_tables(section).next())
        .flatten()
}

fn package_config_table(name: &str) -> Option<&'static str> {
    match name {
        "site" => Some("site"),
        "icon" => Some("icons"),
        _ => None,
    }
}

fn related_packages(section: &str) -> Vec<String> {
    tola_packages::builtin_packages()
        .map(|package| package.spec().name.to_string())
        .filter(|name| package_config_table(name) == Some(section))
        .collect()
}

fn choice_line(label: &str, choices: impl IntoIterator<Item = String>) -> String {
    format!(
        "{label}: {}",
        choices.into_iter().collect::<Vec<_>>().join(", ")
    )
}

fn table_choices() -> String {
    choice_line(
        "Configuration entries",
        TABLES.iter().map(ConfigTable::header),
    )
}

fn package_choices() -> String {
    choice_line(
        "Bundled packages",
        tola_packages::builtin_packages().map(|package| package.spec().name.to_string()),
    )
}

fn quoted(selector: &str) -> String {
    format!("\"{selector}\"")
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

    fn targets(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).into()).collect()
    }

    fn check_refs<'a>(refs: impl IntoIterator<Item = &'a str>, pkg: Option<&str>, ctx: &str) {
        for raw in refs {
            resolve_reference(raw, pkg)
                .unwrap_or_else(|err| panic!("{ctx} broken ref `{raw}`: {err}"));
        }
    }

    fn check_params(
        doc_params: &std::collections::BTreeMap<String, Markup>,
        sig_params: &[tola_packages::Parameter],
        ctx: &str,
    ) {
        for p in doc_params.keys() {
            assert!(
                sig_params.iter().any(|sp| sp.name == *p),
                "{ctx} documents unknown param `{p}`"
            );
        }
        for p in sig_params {
            assert!(
                doc_params.contains_key(&p.name),
                "{ctx} is missing param `{}`",
                p.name
            );
        }
    }

    #[test]
    fn config_docs_match_schema() {
        for lang in [HelpLanguage::English, HelpLanguage::SimplifiedChinese] {
            let tag = lang.tag();
            for section in docs::config_names() {
                assert!(
                    TABLES.iter().any(|t| t.section == section),
                    "`{section}` is not a registered config table"
                );
                let doc = docs::config(lang, section).unwrap();
                let schema_keys: Vec<&str> = documented_fields(section)
                    .into_iter()
                    .filter(|p| child_table(p).is_none())
                    .map(|p| p.rsplit_once('.').map_or(p, |(_, n)| n))
                    .collect();
                for k in doc.keys.keys() {
                    assert!(
                        schema_keys.contains(&k.as_str()),
                        "{tag} config `{section}` documents unknown key `{k}`"
                    );
                }
                for k in &schema_keys {
                    assert!(
                        doc.keys.contains_key(*k),
                        "{tag} config `{section}` is missing docs for key `{k}`"
                    );
                }
                check_refs(
                    doc.summary.refs().into_iter().chain(doc.body.refs()),
                    None,
                    &format!("{tag} config `{section}`"),
                );
                for (name, kdoc) in &doc.keys {
                    check_refs(
                        kdoc.body.refs(),
                        None,
                        &format!("{tag} config `{section}.{name}`"),
                    );
                }
                resolve_related_list(&doc.related, None)
                    .unwrap_or_else(|err| panic!("{tag} config `{section}` broken related: {err}"));
            }
            for table in TABLES {
                assert!(
                    docs::config(lang, table.section).is_some(),
                    "{tag} missing config doc for `{}`",
                    table.section
                );
            }
        }
    }

    #[test]
    fn package_docs_match_declarations() {
        for lang in [HelpLanguage::English, HelpLanguage::SimplifiedChinese] {
            let tag = lang.tag();
            for name in docs::package_names() {
                let pkg = builtin_pkg(name)
                    .unwrap_or_else(|| panic!("`{name}` is not a bundled package"));
                let doc = docs::package(lang, name).unwrap();
                let exp_names: Vec<String> = pkg.exports().map(str::to_owned).collect();
                for dexp in doc.exports.keys() {
                    assert!(
                        exp_names.contains(dexp),
                        "{tag} package `{name}` documents unknown export `{dexp}`"
                    );
                }
                for exp in pkg.export_documentation(&exp_names).unwrap() {
                    let ctx = format!("{tag} `{name}.{}`", exp.name);
                    let edoc = doc.exports.get(&exp.name).unwrap_or_else(|| {
                        panic!("{tag} package `{name}` missing export `{}`", exp.name)
                    });
                    if let ExportDeclaration::Function(sig) = &exp.declaration {
                        check_params(&edoc.params, &sig.parameters, &ctx);
                    } else {
                        assert!(
                            edoc.params.is_empty(),
                            "{ctx} value should not document params"
                        );
                    }
                    for mname in edoc.methods.keys() {
                        assert!(
                            exp.methods.iter().any(|m| m.name == *mname),
                            "{ctx} documents unknown method `{mname}`"
                        );
                    }
                    for m in &exp.methods {
                        let mdoc = edoc
                            .methods
                            .get(&m.name)
                            .unwrap_or_else(|| panic!("{ctx} missing method `{}`", m.name));
                        check_params(&mdoc.params, &m.parameters, &format!("{ctx}.{}", m.name));
                    }
                    check_refs(
                        edoc.summary
                            .refs()
                            .into_iter()
                            .chain(edoc.body.refs())
                            .chain(edoc.params.values().flat_map(Markup::refs)),
                        Some(name),
                        &ctx,
                    );
                    resolve_related_list(&edoc.related, Some(name))
                        .unwrap_or_else(|err| panic!("{ctx} broken related: {err}"));
                }
                let pctx = format!("{tag} package `{name}`");
                check_refs(
                    doc.summary.refs().into_iter().chain(doc.body.refs()),
                    Some(name),
                    &pctx,
                );
                resolve_related_list(&doc.related, Some(name))
                    .unwrap_or_else(|err| panic!("{pctx} broken related: {err}"));
            }
            for pkg in tola_packages::builtin_packages() {
                let spec = pkg.spec();
                let name = spec.name.as_str();
                assert!(
                    docs::package(lang, name).is_some(),
                    "{tag} missing package doc for `{name}`"
                );
            }
        }
    }

    #[test]
    fn script_docs_match_signatures() {
        for lang in [HelpLanguage::English, HelpLanguage::SimplifiedChinese] {
            let tag = lang.tag();
            for name in docs::script_names() {
                let page = super::super::scripts::page(name, None, lang).unwrap();
                let doc = docs::script(lang, name).unwrap();
                for cmd in &page.commands {
                    let ctx = format!("{tag} script `{name}.{}`", cmd.name);
                    let cdoc = doc.commands.get(&cmd.name).unwrap_or_else(|| {
                        panic!("{tag} script `{name}` missing command `{}`", cmd.name)
                    });
                    for param in cmd.parameters.iter().chain(&cmd.options) {
                        assert!(
                            cdoc.params.contains_key(&param.name),
                            "{ctx} missing param `{}`",
                            param.name
                        );
                    }
                    check_refs(
                        cdoc.summary
                            .refs()
                            .into_iter()
                            .chain(cdoc.body.refs())
                            .chain(cdoc.params.values().flat_map(Markup::refs)),
                        None,
                        &ctx,
                    );
                }
            }
        }
    }

    #[test]
    fn catalog_serializes_for_both_languages() {
        for lang in [HelpLanguage::English, HelpLanguage::SimplifiedChinese] {
            let full = catalog(lang).unwrap();
            let json = serde_json::to_value(&full).unwrap();
            assert_eq!(json["kind"], "catalog");
            assert_eq!(json["language"], lang.tag());
            assert!(!full.config.is_empty());
            assert!(!full.packages.is_empty());
            assert!(!full.scripts.is_empty());
        }
    }

    #[test]
    fn wrong_brackets_suggest_selector() {
        for (wrong, header) in [
            ("[hooks.before-build]", "[[hooks.before-build]]"),
            ("[[build]]", "[build]"),
            ("[include]", "include"),
        ] {
            let err = request(&targets(&[wrong])).unwrap_err();
            let diag = &tola_diagnostic::attached(&err).unwrap()[0];
            assert_eq!(diag.code, crate::codes::help::TARGET);
            assert!(
                diag.help
                    .iter()
                    .any(|h| h.message.contains(&quoted(header)))
            );
        }
    }

    #[test]
    fn empty_container_shows_child() {
        let HelpPage::Config(page) = page_for_target(
            &request(&targets(&["[typst]"])).unwrap(),
            HelpLanguage::English,
        )
        .unwrap() else {
            panic!("expected config page");
        };
        assert_eq!(page.section, "typst.fonts");
    }

    #[test]
    fn selected_exports_keep_order() {
        let HelpPage::Package(page) = page_for_target(
            &request(&targets(&["@tola/address", "output-url", "slugify"])).unwrap(),
            HelpLanguage::English,
        )
        .unwrap() else {
            panic!("expected package page");
        };
        let names: Vec<&str> = page.exports.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["output-url", "slugify"]);
    }

    #[test]
    fn unknown_export_rejected() {
        let err = page_for_target(
            &request(&targets(&["@tola/address", "slugify", "unknown"])).unwrap(),
            HelpLanguage::English,
        )
        .expect_err("an unknown export is rejected");
        assert_eq!(
            tola_diagnostic::attached(&err).unwrap()[0].code,
            crate::codes::help::MEMBER
        );
    }
}
