//! URL slugification and path utilities.
//!
//! Converts paths and fragments to URL-safe formats.

use crate::config::{SiteConfig, SlugMode};
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

/// Characters forbidden in file paths and fragments
const FORBIDDEN_CHARS: &[char] = &[
    '<', '>', ':', '|', '?', '*', '#', '\\', '(', ')', '[', ']', '\t', '\r', '\n',
];

// ============================================================================
// Slugification
// ============================================================================

/// Convert fragment text to URL-safe format based on config
pub fn slugify_fragment(text: &str, config: &'static SiteConfig) -> String {
    match config.build.slug.fragment {
        SlugMode::Safe => sanitize_text(text),
        SlugMode::On => slug::slugify(text),
        SlugMode::No => text.to_owned(),
    }
}

/// Convert path to URL-safe format based on config
pub fn slugify_path(path: impl AsRef<Path>, config: &'static SiteConfig) -> PathBuf {
    match config.build.slug.path {
        SlugMode::Safe => sanitize_path(path.as_ref()),
        SlugMode::On => slug::slugify(path.as_ref().to_string_lossy()).into(),
        SlugMode::No => path.as_ref().to_path_buf(),
    }
}

/// Remove forbidden characters and replace whitespace with underscores
fn sanitize_text(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !FORBIDDEN_CHARS.contains(c))
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}

/// Sanitize each component of a path
fn sanitize_path(path: &Path) -> PathBuf {
    path.components()
        .map(|c| sanitize_text(&c.as_os_str().to_string_lossy()))
        .collect()
}

// ============================================================================
// Content Path Utilities
// ============================================================================

/// Computed paths for a content file.
pub struct ContentPaths {
    /// Relative path without `.typ` extension.
    /// Example: `content/posts/hello.typ` → `"posts/hello"`
    pub relative: String,

    /// Full output HTML path (slugified).
    /// Example: `public/posts/hello/index.html`
    pub html: PathBuf,
}

/// Compute output paths for a `.typ` content file.
///
/// This function maps a source `.typ` file to its HTML output location:
/// - Strips the content directory prefix
/// - Removes the `.typ` extension
/// - Applies path slugification
/// - Generates the final HTML path
///
/// # Path Mapping Examples
///
/// | Source | relative | html |
/// |--------|----------|------|
/// | `content/posts/hello.typ` | `posts/hello` | `public/posts/hello/index.html` |
/// | `content/index.typ` | `index` | `public/index.html` |
pub fn content_paths(content_path: &Path, config: &'static SiteConfig) -> Result<ContentPaths> {
    let content_dir = &config.build.content;
    let output_dir = config.build.output.join(&config.build.path_prefix);

    // Strip content dir and .typ extension: "content/posts/hello.typ" → "posts/hello"
    let relative = content_path
        .strip_prefix(content_dir)?
        .to_str()
        .ok_or_else(|| anyhow!("Invalid path encoding"))?
        .strip_suffix(".typ")
        .ok_or_else(|| anyhow!("Not a .typ file: {}", content_path.display()))?
        .to_owned();

    // Special case: root index.typ → public/path_prefix/index.html (not public/path_prefix/index/index.html)
    // Only content/index.typ is the root index, not content/subdir/index.typ
    let is_root_index = relative == "index";

    let html = if is_root_index {
        output_dir.join("index.html")
    } else {
        output_dir.join(&relative).join("index.html")
    };
    let html = slugify_path(html, config);

    Ok(ContentPaths { relative, html })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_text_removes_forbidden_chars() {
        let input = "Hello<World>";
        let result = sanitize_text(input);
        assert_eq!(result, "HelloWorld");
    }

    #[test]
    fn test_sanitize_text_removes_all_forbidden_chars() {
        let input = "a<b>c:d|e?f*g#h\\i(j)k[l]m";
        let result = sanitize_text(input);
        assert_eq!(result, "abcdefghijklm");
    }

    #[test]
    fn test_sanitize_text_replaces_whitespace() {
        let input = "Hello World";
        let result = sanitize_text(input);
        assert_eq!(result, "Hello_World");
    }

    #[test]
    fn test_sanitize_text_replaces_various_whitespace() {
        let input = "Hello\tWorld\nTest";
        let result = sanitize_text(input);
        // \t and \n are forbidden chars, so they are removed
        assert_eq!(result, "HelloWorldTest");
    }

    #[test]
    fn test_sanitize_text_trims() {
        let input = "  Hello World  ";
        let result = sanitize_text(input);
        assert_eq!(result, "Hello_World");
    }

    #[test]
    fn test_sanitize_text_preserves_unicode() {
        let input = "你好世界";
        let result = sanitize_text(input);
        assert_eq!(result, "你好世界");
    }

    #[test]
    fn test_sanitize_text_complex_input() {
        let input = "  Hello (World) [Test]: #anchor?  ";
        let result = sanitize_text(input);
        assert_eq!(result, "Hello_World_Test_anchor");
    }

    #[test]
    fn test_sanitize_path_simple() {
        let path = Path::new("content/posts/hello-world");
        let result = sanitize_path(path);
        assert_eq!(result, PathBuf::from("content/posts/hello-world"));
    }

    #[test]
    fn test_sanitize_path_with_forbidden_chars() {
        let path = Path::new("content/posts/hello<world>");
        let result = sanitize_path(path);
        assert_eq!(result, PathBuf::from("content/posts/helloworld"));
    }

    #[test]
    fn test_sanitize_path_with_spaces() {
        let path = Path::new("content/my posts/hello world");
        let result = sanitize_path(path);
        assert_eq!(result, PathBuf::from("content/my_posts/hello_world"));
    }

    #[test]
    fn test_forbidden_chars_constant() {
        // Verify all expected forbidden characters are present
        assert!(FORBIDDEN_CHARS.contains(&'<'));
        assert!(FORBIDDEN_CHARS.contains(&'>'));
        assert!(FORBIDDEN_CHARS.contains(&':'));
        assert!(FORBIDDEN_CHARS.contains(&'|'));
        assert!(FORBIDDEN_CHARS.contains(&'?'));
        assert!(FORBIDDEN_CHARS.contains(&'*'));
        assert!(FORBIDDEN_CHARS.contains(&'#'));
        assert!(FORBIDDEN_CHARS.contains(&'\\'));
        assert!(FORBIDDEN_CHARS.contains(&'('));
        assert!(FORBIDDEN_CHARS.contains(&')'));
        assert!(FORBIDDEN_CHARS.contains(&'['));
        assert!(FORBIDDEN_CHARS.contains(&']'));
        assert!(FORBIDDEN_CHARS.contains(&'\t'));
        assert!(FORBIDDEN_CHARS.contains(&'\r'));
        assert!(FORBIDDEN_CHARS.contains(&'\n'));
    }

    #[test]
    fn test_sanitize_text_empty_string() {
        let input = "";
        let result = sanitize_text(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_sanitize_text_only_forbidden_chars() {
        let input = "<>:?*#";
        let result = sanitize_text(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_sanitize_text_mixed_content() {
        let input = "My Article (2024) - Part #1";
        let result = sanitize_text(input);
        assert_eq!(result, "My_Article_2024_-_Part_1");
    }
}
