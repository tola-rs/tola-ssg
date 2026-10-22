//! The single built-in starter program.

pub(super) const SITE_PATH: &str = "site.typ";
pub(super) const SCHEMA_PATH: &str = "site/schema.typ";
pub(super) const SCHEMA_SOURCE: &str = include_str!("templates/schema.typ");
pub(super) const PAGE_PATH: &str = "site/page.typ";
pub(super) const PAGE_SOURCE: &str = include_str!("templates/page.typ");
pub(super) const NOT_FOUND_PATH: &str = "site/not-found.typ";
pub(super) const NOT_FOUND_SOURCE: &str = include_str!("templates/not-found.typ");
pub(super) const SELECTION_PATH: &str = "site/selection.typ";
pub(super) const SELECTION_SOURCE: &str = include_str!("templates/selection.typ");
pub(super) const SEO_PATH: &str = "site/seo.typ";
pub(super) const SEO_SOURCE: &str = include_str!("templates/seo.typ");

pub(super) fn site_program() -> String {
    concat!(
        include_str!("templates/site-imports.typ"),
        include_str!("templates/site.typ")
    )
    .to_owned()
}
