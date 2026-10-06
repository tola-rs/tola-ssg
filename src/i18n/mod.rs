//! The language `tola help` writes its pages in, and the packaged translations of those pages.
//!
//! The documentation's own spelling is English: a bundled package's overview and export
//! documentation come from its Typst sources, and the index prose is written in `help.rs`. A
//! translation overrides one unit of that spelling; a unit without one reads English, so a
//! partially translated page stays readable. Page sections and labels are never translated.
//! Nothing outside `tola help` follows this language: commands, their help, and diagnostics stay
//! English.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

/// The language `tola help` writes its pages in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpLanguage {
    English,
    SimplifiedChinese,
}

impl HelpLanguage {
    /// The language one `--lang` or environment value names, when it names one Tola writes.
    ///
    /// A tag is lowercased and stripped of its codeset and modifier, so `zh_CN.UTF-8` and
    /// `zh-Hans-CN` both read `zh`. `C` and `POSIX` name English. Every `zh` variant currently
    /// reads the Simplified catalog; Traditional Chinese waits for its own.
    pub(crate) fn parse(tag: &str) -> Option<Self> {
        let tag = tag.trim();
        let tag = tag.split(['.', '@']).next().unwrap_or(tag);
        let tag = tag.replace('_', "-");
        let language = tag
            .split('-')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match language.as_str() {
            "en" | "c" | "posix" => Some(Self::English),
            "zh" => Some(Self::SimplifiedChinese),
            _ => None,
        }
    }

    /// The language this invocation writes `tola help` in.
    ///
    /// The flag decides. Otherwise the environment follows the gettext lookup order —
    /// `TOLA_LANG`, `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG` — and the first variable that
    /// names anything decides, so an unsupported value reads English rather than reaching a
    /// variable below it. A value Tola writes no catalog for reads English.
    pub(crate) fn resolve(explicit: Option<Self>) -> Self {
        explicit.unwrap_or_else(|| {
            resolve_from_environment(|name| std::env::var(name).ok()).unwrap_or(Self::English)
        })
    }
}

/// The language the environment names, if any.
fn resolve_from_environment(lookup: impl Fn(&str) -> Option<String>) -> Option<HelpLanguage> {
    for name in ["TOLA_LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        let Some(value) = lookup(name) else { continue };
        if value.trim().is_empty() {
            continue;
        }
        if name == "LANGUAGE" {
            return Some(
                value
                    .split(':')
                    .find_map(HelpLanguage::parse)
                    .unwrap_or(HelpLanguage::English),
            );
        }
        return Some(HelpLanguage::parse(&value).unwrap_or(HelpLanguage::English));
    }
    None
}

/// One phrase a `tola help` page writes, translated where the language carries one.
///
/// Only the page's own prose is here; the names it prints — table headers, keys, package and
/// export names, the `Tola help pages:` `Configuration tables:` and `Bundled packages:` labels —
/// stay English in both languages, because they are what the reader types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpText {
    SiteFields,
    PackageOverview,
    SelectedExports,
    DefaultValues,
}

impl HelpText {
    /// The asset key this phrase is translated under.
    fn key(self) -> &'static str {
        match self {
            Self::SiteFields => "site-fields",
            Self::PackageOverview => "package-overview",
            Self::SelectedExports => "selected-exports",
            Self::DefaultValues => "default-values",
        }
    }

    /// The phrase itself, which is what a page without a translation reads.
    fn english(self) -> &'static str {
        match self {
            Self::SiteFields => "Configuration fields and defaults",
            Self::PackageOverview => "Package overview and exports",
            Self::SelectedExports => "Selected exports",
            Self::DefaultValues => {
                "Default values; use `tola config` to inspect your site's settings."
            }
        }
    }
}

/// The text of one phrase in `language`, reading English where no translation exists.
pub(crate) fn text(language: HelpLanguage, phrase: HelpText) -> &'static str {
    if language == HelpLanguage::SimplifiedChinese
        && let Some(translated) = TEXT.get(phrase.key())
    {
        return translated;
    }
    phrase.english()
}

/// The translated documentation of one bundled package.
pub(crate) struct PackageTranslations {
    overview: Option<String>,
    exports: BTreeMap<String, ExportTranslations>,
}

struct ExportTranslations {
    /// The prose between one export's fenced examples, in order.
    summary: Option<Vec<String>>,
    parameters: BTreeMap<String, String>,
}

impl PackageTranslations {
    /// The package's translated overview, when one exists.
    pub(crate) fn overview(&self) -> Option<&str> {
        self.overview.as_deref()
    }

    /// One export's translated summary prose, when one exists.
    pub(crate) fn summary_prose(&self, export: &str) -> Option<&[String]> {
        self.exports.get(export)?.summary.as_deref()
    }

    /// One parameter's translated description, when one exists.
    pub(crate) fn parameter(&self, export: &str, parameter: &str) -> Option<&str> {
        self.exports
            .get(export)?
            .parameters
            .get(parameter)
            .map(String::as_str)
    }

    /// Read one package's asset.
    fn parse(asset: &str) -> Self {
        let asset: PackageAsset =
            toml::from_str(asset).expect("the bundled `tola help` translations must parse");
        Self {
            overview: normalized(asset.overview),
            exports: asset
                .exports
                .into_iter()
                .map(|(name, export)| {
                    let parameters = export
                        .parameters
                        .into_iter()
                        .filter_map(|(name, description)| {
                            Some((name, normalized(Some(description))?))
                        })
                        .collect();
                    (
                        name,
                        ExportTranslations {
                            summary: translated_parts(export.summary),
                            parameters,
                        },
                    )
                })
                .collect(),
        }
    }
}

/// The translated documentation of `name` in `language`, when that language carries one.
pub(crate) fn package(language: HelpLanguage, name: &str) -> Option<&'static PackageTranslations> {
    if language != HelpLanguage::SimplifiedChinese {
        return None;
    }
    PACKAGES.get(name)
}

/// The translated prose of the configuration tables, keyed by the unit the schema spells.
///
/// A section carries its own documentation and the help its page ends with; every declared field
/// carries its meaning, keyed by its full path (`build.entry`).
pub(crate) struct SectionTranslations {
    sections: BTreeMap<String, SectionProse>,
    fields: BTreeMap<String, String>,
}

struct SectionProse {
    documentation: Option<String>,
    help: Option<String>,
}

impl SectionTranslations {
    /// One section's own documentation, translated.
    pub(crate) fn documentation(&self, section: &str) -> Option<&str> {
        self.sections.get(section)?.documentation.as_deref()
    }

    /// The help one section's page ends with, translated.
    pub(crate) fn help(&self, section: &str) -> Option<&str> {
        self.sections.get(section)?.help.as_deref()
    }

    /// One field's meaning, keyed by its full path.
    pub(crate) fn field(&self, path: &str) -> Option<&str> {
        self.fields.get(path).map(String::as_str)
    }

    /// Every section the catalog translates, for the guard that holds it against the schema.
    #[allow(dead_code)] // The renderer walks the schema; only the guard reads the catalog's keys.
    pub(crate) fn section_names(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().map(String::as_str)
    }

    /// Every field path the catalog translates, for the guard that holds it against the schema.
    #[allow(dead_code)] // The renderer walks the schema; only the guard reads the catalog's keys.
    pub(crate) fn field_paths(&self) -> impl Iterator<Item = &str> {
        self.fields.keys().map(String::as_str)
    }
}

/// The configuration tables `language` translates, when that language carries them.
pub(crate) fn sections(language: HelpLanguage) -> Option<&'static SectionTranslations> {
    if language != HelpLanguage::SimplifiedChinese {
        return None;
    }
    Some(&SECTIONS)
}

/// Trim an authored unit; an empty one is no translation.
fn normalized(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Keep the authored summary prose, dropping nothing else.
fn translated_parts(parts: Option<Vec<String>>) -> Option<Vec<String>> {
    let parts = parts?
        .into_iter()
        .map(|part| part.trim().to_owned())
        .collect::<Vec<_>>();
    parts.iter().any(|part| !part.is_empty()).then_some(parts)
}

/// One export's documentation in `prose`, around the fenced examples `english` writes.
///
/// A fenced example is code, so it reads the same in every language: a translation carries the
/// prose and the source carries the examples. Parts that do not line up with the source's
/// examples read English.
pub(crate) fn localize_summary(english: &str, prose: &[String]) -> String {
    use tola_packages::{DocumentationSegment, documentation_segments};

    let segments = documentation_segments(english);
    let examples = segments
        .iter()
        .filter(|segment| matches!(segment, DocumentationSegment::Example(_)))
        .count();
    if prose.len() != examples + 1 {
        return english.to_owned();
    }
    let mut text = String::new();
    let mut next = 0;
    for segment in &segments {
        match segment {
            DocumentationSegment::Prose(_) => {
                text.push_str(prose[next].trim());
                next += 1;
            }
            DocumentationSegment::Example(example) => text.push_str(example.trim_end()),
        }
        text.push_str("\n\n");
    }
    let end = text.trim_end().len();
    text.truncate(end);
    text
}

/// The phrases one `tola help` asset translates, keyed by [`HelpText::key`].
#[derive(Deserialize)]
struct TextAsset {
    text: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct PackageAsset {
    overview: Option<String>,
    #[serde(default)]
    exports: BTreeMap<String, ExportAsset>,
}

#[derive(Deserialize)]
struct ExportAsset {
    summary: Option<Vec<String>>,
    #[serde(default)]
    parameters: BTreeMap<String, String>,
}

/// The configuration tables one `tola help` asset translates.
#[derive(Deserialize)]
struct SectionsAsset {
    #[serde(default)]
    sections: BTreeMap<String, SectionAsset>,
    #[serde(default)]
    fields: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct SectionAsset {
    documentation: Option<String>,
    help: Option<String>,
}

static TEXT: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    let asset: TextAsset = toml::from_str(include_str!("zh-Hans/text.toml"))
        .expect("the bundled `tola help` translations must parse");
    asset.text
});

static PACKAGES: LazyLock<BTreeMap<&'static str, PackageTranslations>> = LazyLock::new(|| {
    [
        ("site", include_str!("zh-Hans/packages/site.toml")),
        ("address", include_str!("zh-Hans/packages/address.toml")),
        ("icon", include_str!("zh-Hans/packages/icon.toml")),
        ("image", include_str!("zh-Hans/packages/image.toml")),
        ("source", include_str!("zh-Hans/packages/source.toml")),
        (
            "collection",
            include_str!("zh-Hans/packages/collection.toml"),
        ),
        ("schema", include_str!("zh-Hans/packages/schema.toml")),
        ("document", include_str!("zh-Hans/packages/document.toml")),
        ("code", include_str!("zh-Hans/packages/code.toml")),
        ("web", include_str!("zh-Hans/packages/web.toml")),
    ]
    .into_iter()
    .map(|(name, asset)| (name, PackageTranslations::parse(asset)))
    .collect()
});

static SECTIONS: LazyLock<SectionTranslations> = LazyLock::new(|| {
    let asset: SectionsAsset = toml::from_str(include_str!("zh-Hans/sections.toml"))
        .expect("the bundled `tola help` translations must parse");
    SectionTranslations {
        sections: asset
            .sections
            .into_iter()
            .map(|(name, section)| {
                (
                    name,
                    SectionProse {
                        documentation: normalized(section.documentation),
                        help: normalized(section.help),
                    },
                )
            })
            .collect(),
        fields: asset
            .fields
            .into_iter()
            .filter_map(|(path, meaning)| Some((path, normalized(Some(meaning))?)))
            .collect(),
    }
});

#[cfg(test)]
mod tests {
    use super::*;
    use tola_packages::{DocumentationSegment, documentation_segments};

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        move |name: &str| pairs.get(name).cloned()
    }

    #[test]
    fn tags_name_the_language_they_spell() {
        for (tag, expected) in [
            ("en", Some(HelpLanguage::English)),
            ("EN-us", Some(HelpLanguage::English)),
            ("C", Some(HelpLanguage::English)),
            ("POSIX", Some(HelpLanguage::English)),
            ("zh", Some(HelpLanguage::SimplifiedChinese)),
            ("zh_CN.UTF-8", Some(HelpLanguage::SimplifiedChinese)),
            ("zh-Hant-TW", Some(HelpLanguage::SimplifiedChinese)),
            ("fr", None),
            ("", None),
        ] {
            assert_eq!(HelpLanguage::parse(tag), expected, "{tag}");
        }
    }

    #[test]
    fn environment_follows_gettext_order() {
        assert_eq!(
            resolve_from_environment(environment(&[("LANG", "zh_CN.UTF-8")])),
            Some(HelpLanguage::SimplifiedChinese)
        );
        assert_eq!(
            resolve_from_environment(environment(&[
                ("LANG", "zh_CN.UTF-8"),
                ("LC_ALL", "fr_FR.UTF-8"),
            ])),
            Some(HelpLanguage::English),
            "a set variable decides without reaching the ones below it"
        );
        assert_eq!(
            resolve_from_environment(environment(&[
                ("LANG", "zh_CN.UTF-8"),
                ("LANGUAGE", "fr:zh"),
            ])),
            Some(HelpLanguage::SimplifiedChinese)
        );
        assert_eq!(
            resolve_from_environment(environment(&[("LANG", "")])),
            None,
            "an empty variable is ignored"
        );
        assert_eq!(resolve_from_environment(environment(&[])), None);
    }

    #[test]
    fn phrases_have_translations() {
        for phrase in [
            HelpText::SiteFields,
            HelpText::PackageOverview,
            HelpText::SelectedExports,
            HelpText::DefaultValues,
        ] {
            // Exhaustive on purpose: a new phrase stops this test compiling until it is listed.
            match phrase {
                HelpText::SiteFields
                | HelpText::PackageOverview
                | HelpText::SelectedExports
                | HelpText::DefaultValues => {}
            }
            let translated = text(HelpLanguage::SimplifiedChinese, phrase);
            assert_ne!(translated, phrase.english(), "{:?}", phrase.key());
        }
        assert_eq!(
            text(HelpLanguage::English, HelpText::SiteFields),
            "Configuration fields and defaults"
        );
    }

    /// Every bundled package carries a translation, every translated unit names a real export or
    /// parameter, every documented unit carries one, and a translated summary supplies exactly the
    /// prose around the source's examples — the examples themselves come from the source.
    #[test]
    fn translations_and_packages_stay_in_step() {
        for (name, translations) in PACKAGES.iter() {
            let name: &str = name;
            let package = tola_packages::builtin_packages()
                .find(|package| package.spec().name.as_str() == name)
                .unwrap_or_else(|| panic!("`{name}` is not a bundled package"));
            let overview = package.overview();
            match (overview.as_deref(), translations.overview()) {
                (Some(_), None) => panic!("`{name}` overview has no translation"),
                (None, Some(_)) => panic!("`{name}` overview translates nothing"),
                _ => {}
            }
            let names = package.exports().map(str::to_owned).collect::<Vec<_>>();
            let exports = package.export_documentation(&names).unwrap();
            for export_name in translations.exports.keys() {
                assert!(
                    names.contains(export_name),
                    "`{name}` translates no export named `{export_name}`"
                );
            }
            for export in &exports {
                match translations.summary_prose(&export.name) {
                    Some(prose) => {
                        assert!(
                            !export.documentation.summary.is_empty(),
                            "`{name}` export `{}` translates nothing",
                            export.name
                        );
                        let examples = documentation_segments(&export.documentation.summary)
                            .iter()
                            .filter(|segment| matches!(segment, DocumentationSegment::Example(_)))
                            .count();
                        assert_eq!(
                            prose.len(),
                            examples + 1,
                            "`{name}.{}` writes one prose part per example, with one more at the end",
                            export.name
                        );
                    }
                    None => panic!(
                        "`{name}` export `{}` has no translated summary",
                        export.name
                    ),
                }
                let documented = match &export.declaration {
                    tola_packages::ExportDeclaration::Function(signature) => signature
                        .parameters
                        .iter()
                        .filter_map(|parameter| {
                            Some((parameter.name.as_str(), parameter.docs.as_deref()?))
                        })
                        .collect::<Vec<_>>(),
                    tola_packages::ExportDeclaration::Value(_) => Vec::new(),
                };
                for (parameter, docs) in &documented {
                    assert!(!docs.trim().is_empty());
                    assert!(
                        translations.parameter(&export.name, parameter).is_some(),
                        "`{name}.{}` parameter `{parameter}` has no translation",
                        export.name
                    );
                }
                if let Some(translated) = translations.exports.get(&export.name) {
                    for parameter in translated.parameters.keys() {
                        assert!(
                            documented
                                .iter()
                                .any(|(name, _)| *name == parameter.as_str()),
                            "`{name}.{}` translates no parameter named `{parameter}`",
                            export.name
                        );
                    }
                }
            }
        }
        for package in tola_packages::builtin_packages() {
            let spec = package.spec();
            let name = spec.name.as_str();
            assert!(
                PACKAGES.contains_key(name),
                "`{name}` is a bundled package with no translated documentation"
            );
        }
    }
}
