//! Page metadata from frontmatter or `#metadata(...) <tola-meta>`.

use serde::Deserialize;

use super::JsonMap;

/// Deserialize tags, treating `null` as empty vec
fn deserialize_tags<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<Vec<String>> = Option::deserialize(deserializer)?;
    Ok(value.unwrap_or_default())
}

/// Page metadata from `#metadata(...) <tola-meta>` in Typst files
/// or frontmatter in Markdown files
///
/// # Standard Fields
///
/// | Field       | Type           | Description                    |
/// |-------------|----------------|--------------------------------|
/// | `title`     | `String`       | Page title                     |
/// | `summary`   | `JsonValue`    | Brief description (raw JSON)   |
/// | `date`      | `String`       | Publication date               |
/// | `update`    | `String`       | Last update date               |
/// | `author`    | `String`       | Author name                    |
/// | `draft`     | `bool`         | Draft status (default: false)  |
/// | `tags`      | `Vec<String>`  | Categorization tags            |
/// | `permalink` | `String`       | Custom URL path (overrides default) |
/// | `aliases`   | `Vec<String>`  | Redirect URLs to this page     |
///
/// # Custom Fields (`extra`)
///
/// Any additional fields are captured in `extra` as raw JSON
/// Content values (objects with `func` field) are preserved for
/// injection via `Inputs::from_json_with_content()`
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PageMeta {
    pub title: Option<String>,
    /// Summary content (raw JSON, may contain Content structure)
    #[serde(default)]
    pub summary: Option<serde_json::Value>,
    pub date: Option<String>,
    #[allow(dead_code)] // Reserved for future use
    pub update: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub draft: bool,
    /// Tags for categorizing the page.
    #[serde(default, deserialize_with = "deserialize_tags")]
    pub tags: Vec<String>,
    /// Custom permalink (overrides default URL path).
    ///
    /// Example: `/archive/2024/hello/` or `/custom-slug/`
    ///
    /// This is an **input** field used to compute the final permalink.
    /// Skipped during serialization - use `StoredPage.permalink` for output.
    #[serde(skip_serializing)]
    pub permalink: Option<String>,
    /// URL aliases that redirect to this page.
    ///
    /// Example: `["/old-url/", "/legacy/post/"]`
    ///
    /// Aliases generate redirect HTML files pointing to the canonical permalink.
    /// They participate in conflict detection but are excluded from RSS/sitemap.
    #[serde(default, skip_serializing)]
    pub aliases: Vec<String>,
    /// Whether to inject global header (styles, scripts, elements).
    ///
    /// Default: `true`. Set to `false` for pages like 404 that need
    /// self-contained styles to avoid relative path issues.
    #[serde(default = "default_true")]
    pub global_header: bool,
    /// Additional user-defined fields (raw JSON, Content preserved).
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

fn default_true() -> bool {
    true
}

impl Default for PageMeta {
    fn default() -> Self {
        Self {
            title: None,
            summary: None,
            date: None,
            update: None,
            author: None,
            draft: false,
            tags: Vec::new(),
            permalink: None,
            aliases: Vec::new(),
            global_header: true, // Default to true
            extra: JsonMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_meta_default() {
        let meta = PageMeta::default();
        assert!(meta.title.is_none());
        assert!(!meta.draft);
        assert!(meta.global_header);
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn test_page_meta_deserialize() {
        let json = r#"{"title": "Hello", "draft": true, "tags": ["rust", "web"]}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Hello"));
        assert!(meta.draft);
        assert_eq!(meta.tags, vec!["rust", "web"]);
    }

    #[test]
    fn test_page_meta_extra_fields() {
        let json = r#"{"title": "Test", "custom_field": "value", "number": 42}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(
            meta.extra.get("custom_field").and_then(|v| v.as_str()),
            Some("value")
        );
        assert_eq!(meta.extra.get("number").and_then(|v| v.as_i64()), Some(42));
    }

    #[test]
    fn test_page_meta_null_tags() {
        let json = r#"{"tags": null}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn test_page_meta_permalink_not_serialized() {
        let meta = PageMeta {
            title: Some("Test".to_string()),
            permalink: Some("/custom/".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(!json.contains("permalink"));
        assert!(json.contains("title"));
    }
}
