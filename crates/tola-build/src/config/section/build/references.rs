//! `[build.references]` severity of unresolved references.

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// The diagnostic level of the check for a link, resource, or fragment in a published page that
/// points at nothing (the check itself always runs).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.references")]
pub struct ReferencesConfig {
    /// A link to another page of the site, such as `<a href="/missing/">`.
    pub navigation: ReferenceLevel,

    /// A subresource a page loads, such as `<img src="/assets/missing.png">`.
    pub resources: ReferenceLevel,

    /// A fragment a link names, such as `<a href="/page/#missing">`.
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
Only two kinds of destination are checked: a relative one (such as `guide/` or `../guide/`,
resolved against the page's URL) and a site-root one (such as `/guide/`, resolved against the site
root). A destination naming a scheme or host is an external link, which this build cannot check —
`https://example.com/guide/`.

The check reads URL attributes (`href`, `src`, and the like) in the final HTML, hand-written HTML
included. It resolves them the way a browser does: a relative destination resolves against the
page's own URL, or against the page's first `<base href>` when it declares one. A `<base href>`
naming another origin points every relative destination of the page at that site: Tola does not
check them, and a warning (`reference.base_href_external_origin`) says the page was left unchecked.
Remove the `<base href>`, or write it as a relative address (`/`, or `/docs/` under a
`site.base-path`).

Each unresolved reference is reported under its category's diagnostic, at that field's level:

- `navigation`: a linked page missing from the output, such as `<a href=\"/missing/\">`, reports
  `reference.navigation_missing`
- `resources`: a resource the page loads missing from the output, such as
  `<img src=\"/assets/missing.png\">`, reports `reference.resource_missing`; a target whose media
  type does not match, such as `<script src=\"/assets/site.css\">`, reports
  `reference.resource_media_mismatch`
- `fragments`: a fragment the link names missing from the target page, such as
  `<a href=\"/guide/#missing\">`, reports `reference.fragment_missing`; when the target page
  itself is missing, the whole reference reports as `navigation`, and the fragment is not reported
  on its own

`#top` is built into browsers: the part of a URL after `#` is its fragment, and the fragment `top`
goes to the top of the page, so the page needs no `id=\"top\"`. A browser text fragment such as
`#:~:text=words`, and a fragment into a non-HTML file such as `/data.json#missing`, are never
reported as `reference.fragment_missing`.

The checks run after every output exists and before anything is published to `publish-dir`,
against this build's complete output: the HTML pages the Typst Bundle compiles, the files
`[assets]` declares, and the files a `generate-outputs` hook writes — a link to a file a hook
writes is valid, and is not reported as missing.

`tola check` runs the same check as `tola build`, `tola dev`, and `tola preview`, without replacing
the publish directory.

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
