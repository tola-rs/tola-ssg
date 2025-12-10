//! Data types for site-wide data.
//!
//! These types are serialized to JSON and exposed to Typst templates.

use serde::Serialize;

/// Metadata for a single page, exposed in `/_data/pages.json`.
///
/// This is the data available to Typst templates when reading the pages index.
#[derive(Debug, Clone, Serialize)]
pub struct PageData {
    /// Page URL path (e.g., "/posts/hello-world/")
    pub url: String,

    /// Page title (from metadata)
    pub title: String,

    /// Optional summary/description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Publication date as ISO 8601 string (e.g., "2024-01-15")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,

    /// Last update date as ISO 8601 string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,

    /// Author name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Tags associated with this page
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Whether this is a draft (not published)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
}

/// Tags index, exposed in `/_data/tags.json`.
///
/// Maps tag names to lists of pages that have that tag.
/// Sorted alphabetically by tag name.
pub type TagsIndex = std::collections::BTreeMap<String, Vec<TaggedPage>>;

/// A page reference within a tag index.
///
/// Contains minimal information for listing pages by tag.
#[derive(Debug, Clone, Serialize)]
pub struct TaggedPage {
    /// Page URL path
    pub url: String,

    /// Page title
    pub title: String,

    /// Publication date as ISO 8601 string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}
