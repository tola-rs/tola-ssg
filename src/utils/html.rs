//! HTML utility functions.
//!
//! Provides common HTML processing functions:
//! - `escape()`, `escape_attr()` - HTML entity escaping
//! - `is_void_element()` - Self-closing elements (br, img, etc.)
//! - `is_raw_text_element()` - Raw text elements (script, style)
//! - `is_block_element()` - Block-level elements (div, p, etc.)
//! - `parse_attributes()` - HTML attribute string parsing

#![allow(dead_code)]

use std::borrow::Cow;

// =============================================================================
// HTML Escaping
// =============================================================================

/// Characters that require HTML escaping.
const ESCAPE_CHARS: [char; 5] = ['<', '>', '&', '"', '\''];

/// Get the HTML entity for a special character.
#[inline]
fn escape_char(c: char) -> Option<&'static str> {
    match c {
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '&' => Some("&amp;"),
        '"' => Some("&quot;"),
        '\'' => Some("&#39;"),
        _ => None,
    }
}

/// Escape HTML special characters in text content.
///
/// Uses `Cow` to avoid allocation when no escaping is needed.
///
/// # Example
/// ```ignore
/// assert_eq!(escape("<script>"), "&lt;script&gt;");
/// assert_eq!(escape("hello"), "hello"); // No allocation
/// ```
#[inline]
pub fn escape(s: &str) -> Cow<'_, str> {
    escape_with(s, &ESCAPE_CHARS)
}

/// Escape HTML attribute values.
///
/// Escapes characters that are special in attribute contexts.
/// Identical to `escape()` but semantically indicates attribute context.
#[inline]
pub fn escape_attr(s: &str) -> Cow<'_, str> {
    escape_with(s, &ESCAPE_CHARS)
}

/// Internal: escape with specified character set.
#[inline]
fn escape_with<'a>(s: &'a str, chars: &[char]) -> Cow<'a, str> {
    if !s.contains(chars) {
        return Cow::Borrowed(s);
    }

    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match escape_char(c) {
            Some(entity) => result.push_str(entity),
            None => result.push(c),
        }
    }
    Cow::Owned(result)
}

/// Unescape HTML entities back to characters.
///
/// Handles common named entities and numeric character references.
pub fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }

    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '&' {
            result.push(c);
            continue;
        }

        // Collect entity
        let mut entity = String::new();
        for c in chars.by_ref() {
            if c == ';' {
                break;
            }
            entity.push(c);
            if entity.len() > 10 {
                // Too long, not a valid entity
                result.push('&');
                result.push_str(&entity);
                entity.clear();
                break;
            }
        }

        if entity.is_empty() {
            result.push('&');
            continue;
        }

        // Decode entity
        match entity.as_str() {
            "lt" => result.push('<'),
            "gt" => result.push('>'),
            "amp" => result.push('&'),
            "quot" => result.push('"'),
            "apos" => result.push('\''),
            "nbsp" => result.push('\u{00A0}'),
            s if s.starts_with('#') => {
                let code = if s.starts_with("#x") || s.starts_with("#X") {
                    u32::from_str_radix(&s[2..], 16).ok()
                } else {
                    s[1..].parse().ok()
                };
                if let Some(c) = code.and_then(char::from_u32) {
                    result.push(c);
                } else {
                    result.push('&');
                    result.push_str(&entity);
                    result.push(';');
                }
            }
            _ => {
                result.push('&');
                result.push_str(&entity);
                result.push(';');
            }
        }
    }

    Cow::Owned(result)
}

// =============================================================================
// Element Classification
// =============================================================================

/// Check if an HTML tag is a void element (self-closing).
///
/// Void elements cannot have children and should be rendered as `<tag/>`.
#[inline]
pub fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Check if tag is a raw text element (content should not be HTML-escaped).
///
/// Per HTML spec: script and style content is "raw text".
#[inline]
pub fn is_raw_text_element(tag: &str) -> bool {
    matches!(tag, "script" | "style")
}

/// Check if tag is an escapable raw text element.
///
/// Per HTML spec: textarea and title are "escapable raw text".
#[inline]
pub fn is_escapable_raw_text_element(tag: &str) -> bool {
    matches!(tag, "textarea" | "title")
}

/// Check if tag is a block-level element.
///
/// Block elements create line breaks and take full width by default.
#[inline]
pub fn is_block_element(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "canvas"
            | "dd"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "noscript"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "tfoot"
            | "ul"
            | "video"
    )
}

/// Check if tag is an inline element.
#[inline]
pub fn is_inline_element(tag: &str) -> bool {
    !is_block_element(tag) && !is_void_element(tag)
}

// =============================================================================
// Attribute Parsing
// =============================================================================

/// Parse HTML-style attributes from a string.
///
/// Input: `viewBox="0 0 100 100" class="foo" disabled`
/// Output: `vec![("viewBox", "0 0 100 100"), ("class", "foo"), ("disabled", "")]`
pub fn parse_attributes(s: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }

        // Read attribute name
        let mut name = String::new();
        name.push(c);
        while let Some(&next) = chars.peek() {
            if next == '=' || next.is_whitespace() {
                break;
            }
            name.push(chars.next().unwrap());
        }

        // Skip whitespace
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }

        // Check for value
        if chars.peek() == Some(&'=') {
            chars.next(); // consume '='

            // Skip whitespace
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }

            // Read value
            let value = if chars.peek() == Some(&'"') || chars.peek() == Some(&'\'') {
                let quote = chars.next().unwrap();
                let mut val = String::new();
                for c in chars.by_ref() {
                    if c == quote {
                        break;
                    }
                    val.push(c);
                }
                val
            } else {
                // Unquoted value (read until whitespace)
                let mut val = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    val.push(chars.next().unwrap());
                }
                val
            };

            attrs.push((name, value));
        } else {
            // Boolean attribute (no value)
            attrs.push((name, String::new()));
        }
    }

    attrs
}

// =============================================================================
// ANSI to HTML Conversion
// =============================================================================

/// Convert ANSI escape sequences to HTML spans.
///
/// Converts color codes like `\x1b[31m` (red) to `<span style="color:...">`.
/// Used for displaying error messages with syntax highlighting in browser overlays.
pub fn ansi_to_html(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    let mut chars = s.chars().peekable();
    let mut open_spans = 0;

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Collect the code number(s)
                let mut code = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_digit() || ch == ';' {
                        code.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                // Consume the terminator (usually 'm')
                if chars.peek() == Some(&'m') {
                    chars.next();
                }

                // Convert ANSI code to HTML
                if let Some(html) = ansi_code_to_html(&code, &mut open_spans) {
                    result.push_str(&html);
                }
            }
        } else if c == '<' {
            result.push_str("&lt;");
        } else if c == '>' {
            result.push_str("&gt;");
        } else if c == '&' {
            result.push_str("&amp;");
        } else {
            result.push(c);
        }
    }

    // Close any remaining spans
    for _ in 0..open_spans {
        result.push_str("</span>");
    }

    result
}

fn ansi_code_to_html(code: &str, open_spans: &mut i32) -> Option<String> {
    // Handle multiple codes separated by ';'
    let codes: Vec<&str> = code.split(';').collect();

    for c in codes {
        match c {
            "0" => {
                // Reset - close all open spans
                let closes = "</span>".repeat(*open_spans as usize);
                *open_spans = 0;
                return Some(closes);
            }
            "1" => {
                *open_spans += 1;
                return Some("<span style=\"font-weight:bold\">".to_string());
            }
            "31" => {
                *open_spans += 1;
                return Some("<span style=\"color:#ff5555\">".to_string()); // Red
            }
            "32" => {
                *open_spans += 1;
                return Some("<span style=\"color:#50fa7b\">".to_string()); // Green
            }
            "33" => {
                *open_spans += 1;
                return Some("<span style=\"color:#f1fa8c\">".to_string()); // Yellow
            }
            "34" => {
                *open_spans += 1;
                return Some("<span style=\"color:#8be9fd\">".to_string()); // Blue/Cyan
            }
            "35" => {
                *open_spans += 1;
                return Some("<span style=\"color:#ff79c6\">".to_string()); // Magenta
            }
            "36" => {
                *open_spans += 1;
                return Some("<span style=\"color:#8be9fd\">".to_string()); // Cyan
            }
            "90" | "37" => {
                *open_spans += 1;
                return Some("<span style=\"color:#6272a4\">".to_string()); // Gray
            }
            _ => {}
        }
    }
    None
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_plain() {
        assert_eq!(escape("hello world"), "hello world");
    }

    #[test]
    fn test_escape_special_chars() {
        assert_eq!(escape("<script>"), "&lt;script&gt;");
        assert_eq!(escape("a & b"), "a &amp; b");
        assert_eq!(escape("say \"hi\""), "say &quot;hi&quot;");
        assert_eq!(escape("it's"), "it&#39;s");
    }

    #[test]
    fn test_escape_mixed() {
        assert_eq!(
            escape("<a href=\"#\">link & text</a>"),
            "&lt;a href=&quot;#&quot;&gt;link &amp; text&lt;/a&gt;"
        );
    }

    #[test]
    fn test_escape_empty() {
        assert_eq!(escape(""), "");
    }

    #[test]
    fn test_escape_attr() {
        assert_eq!(escape_attr("normal"), "normal");
        assert_eq!(escape_attr("a\"b&c"), "a&quot;b&amp;c");
        assert_eq!(escape_attr("it's"), "it&#39;s");
    }

    #[test]
    fn test_unescape() {
        assert_eq!(unescape("hello"), "hello");
        assert_eq!(unescape("&lt;script&gt;"), "<script>");
        assert_eq!(unescape("a &amp; b"), "a & b");
        assert_eq!(unescape("&quot;hi&quot;"), "\"hi\"");
        assert_eq!(unescape("&#39;"), "'");
        assert_eq!(unescape("&#x27;"), "'");
        assert_eq!(unescape("&#65;"), "A");
        assert_eq!(unescape("&nbsp;"), "\u{00A0}");
    }

    #[test]
    fn test_void_elements() {
        assert!(is_void_element("br"));
        assert!(is_void_element("hr"));
        assert!(is_void_element("img"));
        assert!(is_void_element("input"));
        assert!(!is_void_element("div"));
        assert!(!is_void_element("span"));
        assert!(!is_void_element("a"));
    }

    #[test]
    fn test_raw_text_elements() {
        assert!(is_raw_text_element("script"));
        assert!(is_raw_text_element("style"));
        assert!(!is_raw_text_element("div"));
        assert!(!is_raw_text_element("pre"));
    }

    #[test]
    fn test_block_elements() {
        assert!(is_block_element("div"));
        assert!(is_block_element("p"));
        assert!(is_block_element("h1"));
        assert!(is_block_element("ul"));
        assert!(!is_block_element("span"));
        assert!(!is_block_element("a"));
    }

    #[test]
    fn test_parse_attributes() {
        let attrs = parse_attributes(r#"a="1" b='2' c=3 disabled"#);
        assert_eq!(attrs.len(), 4);
        assert_eq!(attrs[0], ("a".to_string(), "1".to_string()));
        assert_eq!(attrs[1], ("b".to_string(), "2".to_string()));
        assert_eq!(attrs[2], ("c".to_string(), "3".to_string()));
        assert_eq!(attrs[3], ("disabled".to_string(), "".to_string()));
    }
}

