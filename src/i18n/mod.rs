//! Markdown prose for `tola help`. The config schema and package declarations supply the
//! identifiers and the English fallback.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HelpLanguage {
    #[default]
    English,
    SimplifiedChinese,
}

impl HelpLanguage {
    /// Accept locale spellings such as `zh_CN.UTF-8` and `zh-Hans-CN` as well as `en` and `zh`.
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
}

pub(crate) fn overview(language: HelpLanguage) -> &'static str {
    match language {
        HelpLanguage::English => include_str!("en/overview.md"),
        HelpLanguage::SimplifiedChinese => include_str!("zh-Hans/overview.md"),
    }
    .trim()
}

pub(crate) const CONFIG_KEYS: &str = "配置项";
pub(crate) const CONFIG_USAGE: &str = "用法";

type Catalog = BTreeMap<&'static str, Translation>;
type Sections<'a> = BTreeMap<&'a str, &'a str>;

/// A Markdown page starts with its title and introduction. Level-two headings name exports or
/// parts of a configuration page. Level-three headings name parameters or configuration keys.
pub(crate) struct Translation {
    intro: &'static str,
    sections: BTreeMap<&'static str, Section>,
}

struct Section {
    body: &'static str,
    fields: Sections<'static>,
}

impl Translation {
    fn parse(markdown: &'static str) -> Self {
        let (title, body) = markdown.split_once('\n').expect("a help page has a title");
        assert!(title.starts_with("# "), "invalid help title `{title}`");
        let (intro, sections) = split_sections(body, HeadingLevel::H2);
        let sections = sections
            .into_iter()
            .map(|(name, markdown)| {
                let (body, fields) = split_sections(markdown, HeadingLevel::H3);
                (name, Section { body, fields })
            })
            .collect();
        Self { intro, sections }
    }

    pub(crate) fn intro(&self) -> Option<&str> {
        nonempty(self.intro)
    }

    pub(crate) fn section(&self, name: &str) -> Option<&str> {
        nonempty(self.sections.get(name)?.body)
    }

    pub(crate) fn field(&self, section: &str, name: &str) -> Option<&str> {
        nonempty(self.sections.get(section)?.fields.get(name)?)
    }

    #[cfg(test)]
    pub(crate) fn sections(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().copied()
    }

    #[cfg(test)]
    pub(crate) fn fields(&self, section: &str) -> impl Iterator<Item = &str> {
        self.sections
            .get(section)
            .into_iter()
            .flat_map(|section| section.fields.keys().copied())
    }
}

fn nonempty(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}

/// Use Markdown offsets so headings inside fenced examples stay in the example.
fn split_sections(markdown: &str, level: HeadingLevel) -> (&str, Sections<'_>) {
    let mut headings = Parser::new(markdown)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::Heading { level: heading, .. }) if heading == level => {
                let title = markdown[range.clone()].lines().next()?;
                title.starts_with('#').then_some((title, range))
            }
            _ => None,
        })
        .peekable();
    let intro = markdown[..headings
        .peek()
        .map_or(markdown.len(), |(_, range)| range.start)]
        .trim();
    let mut sections = BTreeMap::new();
    while let Some((title, range)) = headings.next() {
        let name = title.trim_start_matches('#').trim().trim_matches('`');
        let end = headings
            .peek()
            .map_or(markdown.len(), |(_, next)| next.start);
        let body = markdown[range.end..end].trim();
        assert!(
            sections.insert(name, body).is_none(),
            "duplicate help heading `{name}`"
        );
    }
    (intro, sections)
}

macro_rules! catalog {
    ($directory:literal, $($name:literal),+ $(,)?) => {
        [$(($name, include_str!(concat!("zh-Hans/", $directory, "/", $name, ".md")))),+]
            .into_iter()
            .map(|(name, markdown)| (name, Translation::parse(markdown)))
            .collect()
    };
}

static PACKAGES: LazyLock<Catalog> = LazyLock::new(|| {
    catalog!(
        "packages",
        "address",
        "code",
        "collection",
        "datalog",
        "diagnostic",
        "document",
        "hash",
        "icon",
        "image",
        "module",
        "schema",
        "site",
        "source",
        "uuid",
        "web",
    )
});

static CONFIG: LazyLock<Catalog> = LazyLock::new(|| {
    catalog!(
        "config",
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
        "modules",
        "options",
        "server",
        "site",
        "typst",
        "typst.fonts",
        "vendor",
    )
});

pub(crate) fn package(language: HelpLanguage, name: &str) -> Option<&'static Translation> {
    match language {
        HelpLanguage::English => None,
        HelpLanguage::SimplifiedChinese => PACKAGES.get(name),
    }
}

pub(crate) fn config(language: HelpLanguage, section: &str) -> Option<&'static Translation> {
    match language {
        HelpLanguage::English => None,
        HelpLanguage::SimplifiedChinese => CONFIG.get(section),
    }
}

#[cfg(test)]
pub(crate) fn config_names() -> impl Iterator<Item = &'static str> {
    CONFIG.keys().copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_packages::ExportDeclaration;

    #[test]
    fn package_translations_name_existing_exports_and_parameters() {
        for (name, translation) in PACKAGES.iter() {
            let package = tola_packages::builtin_packages()
                .find(|package| package.spec().name.as_str() == *name)
                .unwrap_or_else(|| panic!("`{name}` is not a bundled package"));
            let names = translation
                .sections()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let exports = package.export_documentation(&names).unwrap();
            for export in exports {
                for field in translation.fields(&export.name) {
                    assert!(
                        matches!(&export.declaration, ExportDeclaration::Function(signature)
                            if signature.parameters.iter().any(|parameter| parameter.name == field)),
                        "`{name}.{}` has no parameter named `{field}`",
                        export.name,
                    );
                }
            }
        }
    }
}
