//! Embedded declarative `.typ` reference documentation for English and Simplified Chinese.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::language::HelpLanguage;
use super::markup::{
    ConfigDoc, OverviewDoc, PackageDoc, ScriptDoc, parse_config, parse_overview, parse_package,
    parse_script,
};

type Pair<T> = (T, T);
type Catalog<T> = BTreeMap<&'static str, Pair<T>>;

static OVERVIEW: LazyLock<Pair<OverviewDoc>> = LazyLock::new(|| {
    (
        parse_overview("en/overview.typ", include_str!("en/overview.typ"))
            .expect("en/overview.typ must parse"),
        parse_overview("zh-Hans/overview.typ", include_str!("zh-Hans/overview.typ"))
            .expect("zh-Hans/overview.typ must parse"),
    )
});

macro_rules! pair_catalog {
    ($dir:literal, $parser:ident, $($name:literal),+ $(,)?) => {
        [
            $(
                (
                    $name,
                    (
                        $parser(
                            concat!("en/", $dir, "/", $name, ".typ"),
                            include_str!(concat!("en/", $dir, "/", $name, ".typ")),
                        )
                        .unwrap_or_else(|err| panic!("failed to parse en/{}/{}.typ: {err}", $dir, $name)),
                        $parser(
                            concat!("zh-Hans/", $dir, "/", $name, ".typ"),
                            include_str!(concat!("zh-Hans/", $dir, "/", $name, ".typ")),
                        )
                        .unwrap_or_else(|err| panic!("failed to parse zh-Hans/{}/{}.typ: {err}", $dir, $name)),
                    ),
                )
            ),+
        ]
        .into_iter()
        .collect()
    };
}

static CONFIG: LazyLock<Catalog<ConfigDoc>> = LazyLock::new(|| {
    pair_catalog!(
        "config",
        parse_config,
        "assets",
        "build",
        "build.minify",
        "build.references",
        "dev",
        "diagnostics",
        "formatter",
        "hooks",
        "hooks.after-publish",
        "hooks.before-build",
        "hooks.generate-outputs",
        "icons",
        "include",
        "modules",
        "options",
        "server",
        "site",
        "typst",
        "typst.fonts",
        "vendor",
    )
});

static PACKAGES: LazyLock<Catalog<PackageDoc>> = LazyLock::new(|| {
    pair_catalog!(
        "packages",
        parse_package,
        "address",
        "code",
        "collection",
        "datalog",
        "diagnostic",
        "document",
        "feed",
        "hash",
        "head",
        "icon",
        "image",
        "module",
        "schema",
        "site",
        "sitemap",
        "source",
        "uuid",
    )
});

static SCRIPTS: LazyLock<Catalog<ScriptDoc>> =
    LazyLock::new(|| pair_catalog!("scripts", parse_script, "context", "diagnostic", "image",));

fn pick<T>(pair: &Pair<T>, language: HelpLanguage) -> &T {
    match language {
        HelpLanguage::English => &pair.0,
        HelpLanguage::SimplifiedChinese => &pair.1,
    }
}

pub(crate) fn overview(language: HelpLanguage) -> &'static OverviewDoc {
    pick(&OVERVIEW, language)
}

pub(crate) fn config(language: HelpLanguage, section: &str) -> Option<&'static ConfigDoc> {
    CONFIG.get(section).map(|pair| pick(pair, language))
}

pub(crate) fn package(language: HelpLanguage, name: &str) -> Option<&'static PackageDoc> {
    PACKAGES.get(name).map(|pair| pick(pair, language))
}

pub(crate) fn script(language: HelpLanguage, name: &str) -> Option<&'static ScriptDoc> {
    SCRIPTS.get(name).map(|pair| pick(pair, language))
}

#[cfg(test)]
pub(crate) fn config_names() -> impl Iterator<Item = &'static str> {
    CONFIG.keys().copied()
}

#[cfg(test)]
pub(crate) fn package_names() -> impl Iterator<Item = &'static str> {
    PACKAGES.keys().copied()
}

#[cfg(test)]
pub(crate) fn script_names() -> impl Iterator<Item = &'static str> {
    SCRIPTS.keys().copied()
}
