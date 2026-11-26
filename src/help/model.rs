//! Typed reference pages assembled for `tola help` and serialized directly to JSON.

use serde::Serialize;

use super::markup::{ExampleCheck, Markup};
pub(crate) use super::target::{HelpCategory, LinkTarget, PageId};

/// A single help page of any kind.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum HelpPage {
    Overview(OverviewPage),
    Index(IndexPage),
    Config(ConfigPage),
    Package(PackagePage),
    Script(ScriptPage),
}

/// Complete offline reference catalog for one language (`tola help --all`).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct HelpCatalog {
    pub kind: &'static str,
    pub language: &'static str,
    pub overview: OverviewPage,
    pub config: Vec<ConfigPage>,
    pub packages: Vec<PackagePage>,
    pub scripts: Vec<ScriptPage>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OverviewPage {
    pub language: &'static str,
    pub title: String,
    pub body: Markup,
    pub categories: Vec<CategoryEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CategoryEntry {
    pub category: &'static str,
    pub title: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct IndexPage {
    pub category: &'static str,
    pub language: &'static str,
    pub title: String,
    pub summary: Markup,
    pub entries: Vec<IndexEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct IndexEntry {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigPage {
    pub section: String,
    pub header: String,
    pub language: &'static str,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub body: Markup,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<ConfigKey>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ConfigChild>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedItem>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigKey {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<&'static str>>,
    pub body: Markup,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigChild {
    pub section: String,
    pub header: String,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PackagePage {
    pub name: String,
    pub spec: String,
    pub title: String,
    pub import: String,
    pub language: &'static str,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub body: Markup,
    pub exports: Vec<PackageExport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedItem>,
    #[serde(skip)]
    pub selected: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PackageExport {
    pub name: String,
    pub kind: &'static str,
    pub declaration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub body: Markup,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<PackageMethod>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedItem>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PackageMethod {
    pub name: String,
    pub declaration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    pub receiver_note: Markup,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub body: Markup,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScriptPage {
    pub name: String,
    pub locator: String,
    pub language: &'static str,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub body: Markup,
    pub commands: Vec<ScriptCommand>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScriptCommand {
    pub name: String,
    pub usage: String,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub summary: Markup,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub body: Markup,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Parameter>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Parameter {
    pub name: String,
    pub kind: &'static str,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<char>,
    #[serde(skip_serializing_if = "Markup::is_empty")]
    pub description: Markup,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Example {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<Markup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Markup>,
    pub lang: String,
    pub code: String,
    #[serde(skip)]
    #[allow(dead_code)]
    pub check: Option<ExampleCheck>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RelatedItem {
    Config { section: String, header: String },
    Package { name: String, title: String },
    Export { package: String, name: String },
    Script { name: String, locator: String },
    Command { script: String, name: String },
}

impl RelatedItem {
    /// Human-readable label shown in terminal and Markdown lists.
    pub(crate) fn display_label(&self) -> String {
        match self {
            Self::Config { header, .. } => header.clone(),
            Self::Package { title, .. } => title.clone(),
            Self::Export { package, name } => format!("@tola/{package} {name}"),
            Self::Script { locator, .. } => locator.clone(),
            Self::Command { script, name } => format!("@tola-scripts/{script} {name}"),
        }
    }
}

/// Normalized slug used for anchors and command lookup.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct Anchor(String);

impl Anchor {
    pub(super) fn from_fragment(fragment: String) -> Self {
        Self(fragment)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The slug of one heading text: runs of separators fold into a single `-`, letters fold to
/// lowercase, and only letters, digits, and `.`/`[`/`]` are kept.
pub(crate) fn anchor(heading: &str) -> Anchor {
    let mut slug = String::new();
    let mut separated = false;
    for character in heading.chars() {
        if character.is_alphanumeric() || matches!(character, '.' | '[' | ']') {
            if separated && !slug.is_empty() {
                slug.push('-');
            }
            separated = false;
            slug.extend(character.to_lowercase());
        } else {
            separated = true;
        }
    }
    Anchor(slug)
}
