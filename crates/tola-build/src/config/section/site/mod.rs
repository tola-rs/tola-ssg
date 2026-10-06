//! `[site]` section configuration.
//!
//! Site identity and values available to templates.
//!
//! # Example
//!
//! ```toml
//! [site]
//! origin = "https://myblog.com"
//! base-path = "/"
//! title = "My Blog"
//! description = "A personal blog"
//! authors = [{ name = "Alice" }]
//! language = "zh-Hans-CN"
//!
//! [site.extra]
//! github = "https://github.com/alice"
//!
//! ```

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use tola_config::Config;

mod language;
pub use language::{LanguageDeclaration, LanguageSubtag, SiteLanguage, UnusableLanguage};

/// A configured site author exposed to Typst templates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteAuthor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Site identity and values available to templates.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site")]
pub struct SiteSectionConfig {
    /// Public address of the site, such as `https://example.com`, with no path. A site without
    /// it publishes relative addresses only.
    pub origin: Option<String>,

    /// Deployment path the site is mounted below, such as `/docs/`. Begins and ends with `/`.
    #[serde(rename = "base-path")]
    #[config(name = "base-path")]
    pub base_path: String,

    /// The name the site calls itself, in browser tabs and feeds.
    pub title: String,

    /// People credited as the site's authors, available to templates as `site.authors`. Each is
    /// `{ name = "…", email = "…", url = "…" }`.
    #[config(collection = inline)]
    pub authors: Vec<SiteAuthor>,

    /// The summary feeds and search engines show for the site.
    pub description: String,

    /// The languages a multilingual site publishes, written as `["en", "zh-Hans"]` or with the
    /// parts of a tag: `[{ lang = "en" }, { lang = "zh", script = "Hans" }]`. Each entry takes
    /// the shape `language` takes, and is validated the same way. Declaring it makes `language`
    /// the default: `language` must then be written, and must name one of the entries.
    ///
    /// Tola reads this as data and does not act on it. How a site routes, pairs, and switches
    /// between its languages is the site's own to decide.
    #[config(collection = inline)]
    pub languages: Vec<LanguageDeclaration>,

    /// A tag such as `"zh-Hant-TW"`, or `{ lang = "zh", script = "Hant", region = "TW" }`.
    /// Templates read `site.language.lang` (primary language), `.script` (writing system),
    /// `.region`, and `.tag` (combined). Typst text uses the parts; HTML `lang` uses the tag.
    /// Pass `.lang` to `slugify` for Han pronunciation in ASCII mode: `ja` selects Japanese,
    /// other languages select Chinese. Script and region do not change slugification.
    pub language: LanguageDeclaration,

    /// The rights notice templates can print, such as `© 2026 Alice`.
    pub copyright: String,

    /// Your own values, available to templates as `site.extra`.
    #[config(hidden)]
    pub extra: FxHashMap<String, toml::Value>,
}

impl Default for SiteSectionConfig {
    fn default() -> Self {
        Self {
            origin: None,
            base_path: "/".into(),
            title: String::new(),
            authors: Vec::new(),
            description: String::new(),
            languages: Vec::new(),
            language: LanguageDeclaration::default(),
            copyright: String::new(),
            extra: FxHashMap::default(),
        }
    }
}

impl SiteSectionConfig {
    /// What `tola help "[site]"` adds under its table.
    pub const HELP: &'static str = "\
A template reads these as `site.<key>` (`tola help \"@tola/site\"` documents the object):

```toml
[site]
origin = \"https://example.com\"    # The public address, with no path
base-path = \"/docs/\"              # The path the site is served below
title = \"Example\"
description = \"Notes on Typst, published with Tola\"
language = \"zh-Hant-TW\"           # Or { lang = \"zh\", script = \"Hant\", region = \"TW\" }
copyright = \"© 2026 Alice\"
authors = [{ name = \"Alice\", email = \"alice@example.com\" }]
```

`origin` and `base-path` decide every published address: this site serves `guide/index.html` at
`https://example.com/docs/guide/`. Without `origin` the site publishes relative addresses only,
which is why a feed or sitemap asks for one — every address they list is absolute. `tola dev`
serves the same mount locally, such as `http://127.0.0.1:5277/docs/`; `--origin` and `--base-path`
override the pair for one run.

Those two keys also produce the derived `site.url`: the site's canonical address, or `none` when
no `origin` is set. `site.language` carries the whole tag as `tag` and its parts as `lang`,
`script`, and `region` — `zh`, `Hant`, and `TW` for the language above — and the parts are what
`#set text` takes:

```typst
#import \"@tola/site:0.0.0\": site
#let url = site.url                     // none when `origin` is unset
#let tag = site.language.tag            // \"zh-Hant-TW\"
#let script = site.language.script      // \"Hant\", or `auto` when none
#let names = site.authors.map(author => author.name)
```";

    /// `site.language` and `site.languages`, which the site's own templates read.
    ///
    /// `language` is the site's default language. `languages` is a multilingual site's
    /// declaration of the languages it publishes, and Tola only reports what it holds — it
    /// routes nothing itself. When it is declared, `language` must be one of the entries.
    pub(crate) fn validate_language(&self, diagnostics: &mut crate::config::ConfigDiagnostics) {
        let declared = match SiteLanguage::parse(&self.language) {
            Ok(language) => Some(language),
            Err(error) => {
                diagnostics.error(
                    Self::FIELDS.language,
                    language_message("site.language", &error),
                );
                None
            }
        };

        let mut entries = Vec::with_capacity(self.languages.len());
        for (index, language) in self.languages.iter().enumerate() {
            match SiteLanguage::parse(language) {
                Ok(language) => entries.push(language),
                Err(error) => diagnostics.error(
                    Self::FIELDS.languages,
                    language_message(&format!("site.languages[{index}]"), &error),
                ),
            }
        }

        if let Some(declared) = declared
            && !entries.is_empty()
            && !entries.contains(&declared)
        {
            diagnostics.error_with_help(
                Self::FIELDS.languages,
                "`site.language` is not one of `site.languages`",
                "`site.language` names the site's default language, so list it among `site.languages`",
            );
        }
    }

    /// The validated site language.
    ///
    /// Configuration validation rejects an unusable language before any build reads it.
    pub(crate) fn language(&self) -> SiteLanguage {
        SiteLanguage::parse(&self.language).expect("the site language was validated")
    }

    /// The validated languages a multilingual site publishes.
    pub(crate) fn languages(&self) -> impl Iterator<Item = SiteLanguage> + '_ {
        self.languages
            .iter()
            .map(|language| SiteLanguage::parse(language).expect("languages were validated"))
    }

    pub(crate) fn validate_address(&self, diagnostics: &mut crate::config::ConfigDiagnostics) {
        if let Some(origin) = self.origin.as_deref() {
            match tola_address::SiteOrigin::parse(origin) {
                Ok(address) if address.base_path().is_none() => {}
                // The deployment path is a separate setting, so the origin has none.
                Ok(_) => diagnostics.error_with_help(
                    Self::FIELDS.origin,
                    "`site.origin` contains a path",
                    "write the path in `site.base-path` instead",
                ),
                Err(reason) => diagnostics.error_with_help(
                    Self::FIELDS.origin,
                    format!("`site.origin` {reason}"),
                    "Use an absolute `http://` or `https://` origin that names a host",
                ),
            }
        }

        if let Err(error) = tola_address::SiteUrlMount::from_base_path(&self.base_path) {
            diagnostics.error_with_help(
                Self::FIELDS.base_path,
                base_path_message(&self.base_path, &error),
                base_path_help(&error),
            );
        }
    }

    pub(crate) fn normalize_address(&mut self) -> tola_address::SiteUrlMount {
        if let Some(origin) = self.origin.as_deref() {
            self.origin = Some(
                tola_address::SiteOrigin::parse(origin)
                    .expect("site origin was validated")
                    .origin()
                    .to_owned(),
            );
        }
        let mount = tola_address::SiteUrlMount::from_base_path(&self.base_path)
            .expect("site base path was validated");
        self.base_path = mount.base_path();
        mount
    }

    pub(crate) fn url(&self) -> Option<String> {
        self.origin
            .as_ref()
            .map(|origin| format!("{origin}{}", self.base_path))
    }
}

/// What one refused language declaration says, naming the option it was written at.
fn language_message(option: &str, error: &UnusableLanguage) -> String {
    match error {
        UnusableLanguage::Tag { .. } => format!("`{option}` {error}"),
        UnusableLanguage::Subtag { key, .. } => format!("`{option}.{}` {error}", key.spelling()),
    }
}

fn base_path_message(base_path: &str, error: &tola_address::SiteUrlMountError) -> String {
    use tola_address::{RESERVED_ROOT, SiteUrlMountError};
    match error {
        SiteUrlMountError::ReservedRoot => format!(
            "`site.base-path` `{base_path}` is inside `{RESERVED_ROOT}`, the namespace Tola reserves"
        ),
        SiteUrlMountError::MissingTrailingSlash | SiteUrlMountError::InvalidBasePath(_) => {
            format!("`site.base-path` `{base_path}` is not a usable deployment path")
        }
    }
}

fn base_path_help(error: &tola_address::SiteUrlMountError) -> String {
    use tola_address::{RESERVED_ROOT, SiteUrlMountError};
    match error {
        SiteUrlMountError::ReservedRoot => {
            format!("Choose a deployment path outside `{RESERVED_ROOT}`, such as `/docs/`")
        }
        SiteUrlMountError::MissingTrailingSlash => "End it with `/`, as in `/docs/`".to_owned(),
        SiteUrlMountError::InvalidBasePath(inner) => url_path_help(inner).to_owned(),
    }
}

/// The action that turns one refused URL path into a usable deployment path.
fn url_path_help(error: &tola_address::UrlPathError) -> &'static str {
    use tola_address::UrlPathError;
    match error {
        UrlPathError::Empty => "Write a path such as `/docs/`",
        UrlPathError::MissingRoot => "Start it with `/`, as in `/docs/`",
        UrlPathError::Authority => "Start it with a single `/`, as in `/docs/`",
        UrlPathError::SurroundingWhitespace => "Remove the whitespace",
        UrlPathError::QueryOrFragment => "Remove the query string or fragment",
        UrlPathError::InvalidPercentEncoding => "Fix the percent encoding",
        UrlPathError::InvalidUtf8 => "Fix the percent encoding so it spells UTF-8",
        UrlPathError::EncodedSeparator => "Write `/` or `\\` literally, not percent-encoded",
        UrlPathError::Backslash => "Use `/` instead of `\\`",
        UrlPathError::Nul => "Remove the NUL character",
        UrlPathError::Control => "Remove the control characters",
        UrlPathError::EmptySegment => "Remove the empty path segment",
        UrlPathError::DotSegment => "Remove the `.` or `..` path segments",
        UrlPathError::FilesystemPrefix => "Remove the filesystem prefix",
        UrlPathError::FilesystemUnsafeCharacter => {
            "Remove the character a published path cannot contain"
        }
        UrlPathError::FilesystemTrailingDotOrSpace => "Remove the trailing dot or space",
        UrlPathError::WindowsDeviceName => "Rename the segment that is a Windows device name",
        UrlPathError::OutputSegmentTooLong { .. } => "Shorten the path segment",
    }
}

#[cfg(test)]
mod tests {
    use super::SiteSectionConfig;

    #[test]
    fn site_address_normalizes_from_toml() {
        let mut site: SiteSectionConfig =
            toml::from_str("origin = \"https://EXAMPLE.com:443/\"\nbase-path = \"/文档/\"")
                .unwrap();
        let mut diagnostics = crate::config::ConfigDiagnostics::new();
        site.validate_address(&mut diagnostics);
        diagnostics.into_result().unwrap();

        let mount = site.normalize_address();

        assert_eq!(site.origin.as_deref(), Some("https://example.com"));
        assert_eq!(site.base_path, "/%E6%96%87%E6%A1%A3/");
        assert_eq!(
            site.url().as_deref(),
            Some("https://example.com/%E6%96%87%E6%A1%A3/")
        );
        assert_eq!(mount.as_str(), "%E6%96%87%E6%A1%A3");
    }

    #[test]
    fn site_address_forms_are_refused() {
        for source in [
            "origin = \"https://example.com/docs/\"",
            "origin = \"ftp://example.com\"",
            "origin = \"https://user@example.com\"",
            "origin = \"https://@example.com\"",
            "origin = \"https:example.com\"",
            "origin = \"https://example.com/.\"",
            "origin = \"https://example.com/%2e\"",
            "origin = \"https://example.com?preview=1\"",
            "origin = \"https://example.com#preview\"",
            "origin = \" https://example.com\"",
            "base-path = \"docs/\"",
            "base-path = \"/docs\"",
        ] {
            let site: SiteSectionConfig = toml::from_str(source).unwrap();
            let mut diagnostics = crate::config::ConfigDiagnostics::new();
            site.validate_address(&mut diagnostics);
            assert!(diagnostics.into_result().is_err(), "{source}");
        }
    }

    #[test]
    fn reserved_base_path_names_the_namespace() {
        let site: SiteSectionConfig = toml::from_str("base-path = \"/_tola/\"").unwrap();
        let mut diagnostics = crate::config::ConfigDiagnostics::new();
        site.validate_address(&mut diagnostics);

        let errors = diagnostics.errors();
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].message.contains("`site.base-path`")
                && errors[0].message.contains("inside `_tola`"),
            "{}",
            errors[0].message
        );
        assert!(
            errors[0]
                .help
                .as_deref()
                .is_some_and(|help| help.contains("outside `_tola`")),
            "{:?}",
            errors[0].help
        );
    }

    #[test]
    fn language_tags_are_canonicalized() {
        for (source, tag) in [
            ("language = \"ZH-hant-tw\"", "zh-Hant-TW"),
            (
                "language = { lang = \"zh\", script = \"hant\", region = \"TW\" }",
                "zh-Hant-TW",
            ),
        ] {
            let site: SiteSectionConfig = toml::from_str(source).unwrap();
            let mut diagnostics = crate::config::ConfigDiagnostics::new();
            site.validate_language(&mut diagnostics);
            diagnostics.into_result().unwrap();
            assert_eq!(site.language().tag(), tag, "{source}");
        }
    }

    #[test]
    fn subtag_failure_names_its_option() {
        let site: SiteSectionConfig =
            toml::from_str("language = { lang = \"zh\", region = \"Hans\" }").unwrap();
        let mut diagnostics = crate::config::ConfigDiagnostics::new();
        site.validate_language(&mut diagnostics);

        let errors = diagnostics.errors();
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].message.contains("`site.language.region`")
                && errors[0].message.contains("`Hans`")
                && errors[0].message.contains("two-letter"),
            "{}",
            errors[0].message
        );
    }
}
