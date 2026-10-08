//! `[build.references]` severity of unresolved references.

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// How Tola reports a link, resource, or fragment in a published page that points at nothing.
///
/// `error` fails the build; `warn` reports the reference without failing the build.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.references")]
pub struct ReferencesConfig {
    /// A link to another page of the site.
    pub navigation: ReferenceLevel,

    /// An image, stylesheet, or script a page loads.
    pub resources: ReferenceLevel,

    /// A fragment the destination page does not define.
    pub fragments: ReferenceLevel,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceLevel {
    #[default]
    Error,
    Warn,
}

impl ReferencesConfig {
    /// What `tola help config build.references` adds under its table.
    pub const HELP: &'static str = "\
A published page is checked for addresses that land on nothing: `navigation` covers links to
other pages, `resources` the images, stylesheets, and scripts a page loads, and `fragments`
anchors the destination page does not define. `error` fails the build; `warn` reports the
reference and keeps going. Every key defaults to `error`:

```toml
[build.references]
navigation = \"warn\"
resources = \"error\"
fragments = \"warn\"
```

Only a relative or site-root destination is checked; one naming its own scheme and host is left
alone, `site.origin` included, because another site may serve it.

The check reads URL attributes in the final HTML, including hand-written HTML, and resolves them
under the document's first `<base href>`. A `<base href>` naming another origin makes the whole
page another site's, so its relative references are not checked and the build says so. Each
resolved address is compared against the site's complete output, so a target any producer writes
counts.
Missing targets and incompatible resource types use `navigation` or `resources`; only a missing
HTML anchor uses `fragments`. `#top` needs no declared id. Browser fragment directives, such as
`#:~:text=words`, and fragments of non-HTML resources are not checked as HTML ids.

`references()` from `@tola/document` is a separate query over native Bundle links and refs. It
does not include raw HTML or other producers' outputs, and `found` does not guarantee that the
final HTML defines the fragment (`tola help package document references`).";
}

#[cfg(test)]
mod tests {
    use super::{ReferenceLevel, ReferencesConfig};

    #[test]
    fn reference_levels_decode_from_toml() {
        let config: ReferencesConfig = toml::from_str(
            r#"navigation = "warn"
resources = "error"
fragments = "warn""#,
        )
        .unwrap();

        assert_eq!(config.navigation, ReferenceLevel::Warn);
        assert_eq!(config.resources, ReferenceLevel::Error);
        assert_eq!(config.fragments, ReferenceLevel::Warn);
    }
}
