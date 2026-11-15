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
//! ```

use serde::{Deserialize, Serialize};
use tola_config::{Config, Fragment};

mod language;
mod resolved;
pub use language::{LanguageDeclaration, LanguageSubtag, SiteLanguage, UnusableLanguage};
pub use resolved::ResolvedSiteSection;

/// A configured site author exposed to Typst templates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Fragment)]
#[serde(deny_unknown_fields)]
pub struct SiteAuthor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Set the site's address, title, and language. To deploy at
/// `https://example.com/docs/`, add this to `tola.toml`:
///
/// ```toml
/// [site]
/// origin = "https://example.com"
/// base-path = "/docs/"
/// title = "Typst Notes"
/// language = "zh-Hant-TW"
/// ```
///
/// In a site created with `tola init`, write `= Guide` in `content/guide/index.typ`.
/// Run `tola build` from the site root. The file is `public/guide/index.html`. Deploy
/// `public/` under `/docs/` to serve it at `https://example.com/docs/guide/`.
/// With `tola dev` on the default port, open `http://127.0.0.1:5277/docs/guide/`.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site")]
pub struct SiteSectionConfig {
    /// Scheme and host, plus a port if needed. For example, `https://example.com`. Put the path
    /// in `base-path`. If omitted, `site.url` is `none`. Feeds and sitemaps need `origin`.
    pub origin: Option<String>,

    /// The path your site is served under, such as `/docs/`. It starts and ends with `/`.
    /// The default is `/`.
    #[serde(rename = "base-path")]
    #[config(name = "base-path")]
    pub base_path: String,

    /// Site name used by templates and feeds. If omitted, use an empty string.
    pub title: String,

    /// Site authors. For example, `[{ name = "Alice", email = "alice@example.com" }]`.
    /// Each author needs a `name`. `email` and `url` are optional. If omitted, use an empty list.
    #[config(collection = inline)]
    pub authors: Vec<SiteAuthor>,

    /// Short description for feeds and page metadata. If omitted, use an empty string.
    pub description: String,

    /// Languages your templates can use to build a multilingual site. Each entry uses the same
    /// format as `language`, for example `["en", "zh-Hans"]`. If you set a nonempty list, include
    /// the default `language`. If omitted, use an empty list. Your templates decide which pages
    /// and links to generate.
    #[config(collection = inline)]
    pub languages: Vec<LanguageDeclaration>,

    /// The default language (`"en"` if omitted). Write a BCP 47 tag such as `"zh-Hant-TW"` or
    /// the equivalent `{ lang = "zh", script = "Hant", region = "TW" }`.
    pub language: LanguageDeclaration,
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
        }
    }
}

impl SiteSectionConfig {
    /// What `tola help "[site]"` adds under its table.
    pub const HELP: &'static str = "\
### Use the settings in Typst

Read the settings through `@tola/site`. The `site.typ` created by `tola init` sets the text
language like this.

```typst
#import \"@tola/site:0.0.0\": site
#set text(lang: site.language.lang, script: site.language.script, region: site.language.region)
```

If you write the HTML `lang` attribute yourself, use `site.language.tag` for the full tag
(`zh-Hant-TW` above). For `slugify` in ASCII mode, pass `site.language.lang` as `language`.
It uses Japanese readings for Han characters with `ja` and Chinese readings otherwise.

With the settings above, `site.url` is `https://example.com/docs/`. It is `none` when `origin`
is unset. Use `--origin` or `--base-path` to override a setting for one run.";

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
        let site: SiteSectionConfig =
            toml::from_str("origin = \"https://EXAMPLE.com:443/\"\nbase-path = \"/文档/\"")
                .unwrap();
        let mut diagnostics = crate::config::ConfigDiagnostics::new();
        site.validate_address(&mut diagnostics);
        diagnostics.into_result().unwrap();

        let site = site.into_resolved();

        assert_eq!(site.origin.as_deref(), Some("https://example.com"));
        assert_eq!(site.base_path.base_path(), "/%E6%96%87%E6%A1%A3/");
        assert_eq!(
            site.url().as_deref(),
            Some("https://example.com/%E6%96%87%E6%A1%A3/")
        );
        assert_eq!(site.base_path.as_str(), "%E6%96%87%E6%A1%A3");
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
}
