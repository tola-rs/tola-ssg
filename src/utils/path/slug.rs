//! URL slugification and path sanitization utilities.
//!
//! This module provides functions to convert text and file paths into URL-safe formats.
//! It supports multiple slug modes and case transformations.
//!
//! # Slug Modes
//!
//! | Mode | Unicode | Forbidden Chars | Case | Example |
//! |------|---------|-----------------|------|---------|
//! | `On` | -> ASCII | -> separator | lowercase | `"Café World"` -> `"cafe-world"` |
//! | `Safe` | preserved | -> separator | configurable | `"Café World"` -> `"Café-World"` |
//! | `Ascii` | -> ASCII | -> separator | configurable | `"Café World"` -> `"Cafe-World"` |
//! | `No` | preserved | preserved | preserved | `"Café World"` -> `"Café World"` |
//!
//! # Forbidden Characters
//!
//! The following characters are replaced with the separator:
//! `< > : | ? * # \ ( ) [ ] \t \r \n`
//!
//! Consecutive forbidden characters and whitespace are collapsed into a single separator.
//!
//! # Examples
//!
//! ```ignore
//! // Safe mode: preserves Unicode, replaces forbidden chars
//! sanitize_text("Chapter:One", '-') // -> "Chapter-One"
//! sanitize_text("A::::B", '-')    // -> "A-B" (consecutive collapsed)
//!
//! // Full slugify: converts to ASCII lowercase
//! slugify_on("München", '-')       // -> "munchen"
//! ```

use crate::config::{SlugCase, SlugConfig, SlugMode};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// Characters that are unsafe for URLs and file paths
///
/// These characters are replaced with the configured separator
/// Consecutive occurrences are collapsed into a single separator
pub const FORBIDDEN_CHARS: &[char] = &[
    '<', '>', ':', '|', '?', '*', '#', '\\', '(', ')', '[', ']', '\t', '\r', '\n',
];

// ============================================================================
// Public API
// ============================================================================

/// Converts fragment text (e.g., heading anchors) to URL-safe format
///
/// # Arguments
/// * `text` - The text to slugify
/// * `slug` - Slug configuration
///
/// # Example
/// ```ignore
/// // With SlugMode::Safe, separator='-', case=Lower
/// slugify_fragment("Hello World", &slug) // -> "hello-world"
/// slugify_fragment("Chapter:One", &slug) // -> "Chapter-One"
/// ```
pub fn slugify_fragment(text: &str, slug: &SlugConfig) -> String {
    let sep = slug.separator.as_char();

    let result = match slug.fragment {
        SlugMode::No => return text.to_owned(),
        SlugMode::Full => slugify_full(text, sep),
        SlugMode::Safe => sanitize(text, sep),
        SlugMode::Ascii => sanitize(&deunicode::deunicode(text), sep),
    };

    apply_case(&result, &slug.case).into_owned()
}

/// Converts a file path to URL-safe format
///
/// Each path component is processed independently, preserving the directory structure
///
/// # Arguments
/// * `path` - The path to slugify
/// * `slug` - Slug configuration
///
/// # Example
/// ```ignore
/// // With SlugMode::Safe, separator='-', case=Lower
/// slugify_path("content/My Posts/Hello World", &slug)
/// // -> "content/my-posts/hello-world"
/// ```
pub fn slugify_path(path: impl AsRef<Path>, slug: &SlugConfig) -> PathBuf {
    let sep = slug.separator.as_char();

    match slug.path {
        SlugMode::No => path.as_ref().to_path_buf(),
        // Full mode: process each component with full slugification (ASCII + lowercase)
        SlugMode::Full => transform_path_components_full(path.as_ref(), sep),
        SlugMode::Safe => transform_path_components(path.as_ref(), sep, &slug.case, false),
        SlugMode::Ascii => transform_path_components(path.as_ref(), sep, &slug.case, true),
    }
}

// ============================================================================
// Core Transformation Functions
// ============================================================================

/// Full slugification: Unicode -> ASCII, lowercase, separator-delimited
///
/// This is the most aggressive transformation, suitable for URL slugs
/// Always produces lowercase output regardless of case settings
///
/// # Processing Steps
/// 1. Transliterate Unicode to ASCII (via `deunicode`)
/// 2. Convert to lowercase
/// 3. Replace forbidden chars and whitespace with separator
/// 4. Collapse consecutive separators
/// 5. Trim leading/trailing separators
///
/// # Examples
/// ```ignore
/// slugify_full("Hello World", '-')  // -> "hello-world"
/// slugify_full("München", '-')      // -> "munchen"
/// slugify_full("Café Naïve", '-')   // -> "cafe-naive"
/// slugify_full("a:::b", '-')        // -> "a-b"
/// ```
fn slugify_full(text: &str, sep: char) -> String {
    let ascii = deunicode::deunicode(text);
    let replaced = replace_special_chars(&ascii.to_lowercase(), sep);
    collapse_consecutive_separators(&replaced, sep)
}

/// Sanitizes text by replacing forbidden characters with separator
///
/// Preserves Unicode characters while making the text URL-safe
///
/// # Processing Steps
/// 1. Trim leading/trailing whitespace
/// 2. Replace forbidden chars and whitespace with separator
/// 3. Collapse consecutive separators
/// 4. Trim leading/trailing separators
///
/// # Examples
/// ```ignore
/// sanitize("Hello World", '-')   // -> "Hello-World"
/// sanitize("Café#World", '-')      // -> "Café-World"
/// sanitize("a:::b   c", '-')     // -> "a-b-c"
/// sanitize("Chapter:One", '-')   // -> "Chapter-One"
/// ```
fn sanitize(text: &str, sep: char) -> String {
    let replaced = replace_special_chars(text.trim(), sep);
    collapse_consecutive_separators(&replaced, sep)
}

/// Transforms each component of a path with full slugification
///
/// Applies `slugify_full` to each path component individually,
/// preserving the directory structure while fully slugifying each part
fn transform_path_components_full(path: &Path, sep: char) -> PathBuf {
    path.components()
        .map(|component| slugify_full(&component.as_os_str().to_string_lossy(), sep))
        .collect()
}

/// Transforms each component of a path independently
///
/// # Arguments
/// * `path` - The path to transform
/// * `sep` - Separator character
/// * `case` - Case transformation to apply
/// * `to_ascii` - Whether to transliterate Unicode to ASCII
fn transform_path_components(path: &Path, sep: char, case: &SlugCase, to_ascii: bool) -> PathBuf {
    path.components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy();
            let sanitized = if to_ascii {
                sanitize(&deunicode::deunicode(&text), sep)
            } else {
                sanitize(&text, sep)
            };
            apply_case(&sanitized, case).into_owned()
        })
        .collect()
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Replaces forbidden characters and whitespace with the separator
#[inline]
fn replace_special_chars(text: &str, sep: char) -> String {
    text.chars()
        .map(|c| {
            if FORBIDDEN_CHARS.contains(&c) || c.is_whitespace() {
                sep
            } else {
                c
            }
        })
        .collect()
}

/// Collapses consecutive separators into one and trims leading/trailing separators
///
/// # Examples
/// ```ignore
/// collapse_consecutive_separators("a--b--c", '-') // -> "a-b-c"
/// collapse_consecutive_separators("--abc--", '-') // -> "abc"
/// collapse_consecutive_separators("------", '-')  // -> ""
/// ```
fn collapse_consecutive_separators(text: &str, sep: char) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_was_sep = true; // Skip leading separators

    for c in text.chars() {
        if c == sep {
            if !prev_was_sep {
                result.push(c);
                prev_was_sep = true;
            }
            // Skip consecutive separators
        } else {
            result.push(c);
            prev_was_sep = false;
        }
    }

    // Remove trailing separator
    if result.ends_with(sep) {
        result.pop();
    }

    result
}

/// Applies case transformation to text
///
/// # Case Modes
/// - `Lower`: all lowercase
/// - `Upper`: ALL UPPERCASE
/// - `Capitalize`: Title Case (Each Word Capitalized)
/// - `Preserve`: no change
///
/// Uses `Cow` to avoid allocation for `Preserve` mode
fn apply_case<'a>(text: &'a str, case: &SlugCase) -> Cow<'a, str> {
    match case {
        SlugCase::Lower => Cow::Owned(text.to_lowercase()),
        SlugCase::Upper => Cow::Owned(text.to_uppercase()),
        SlugCase::Capitalize => Cow::Owned(capitalize_words(text)),
        SlugCase::Preserve => Cow::Borrowed(text),
    }
}

/// Capitalizes the first letter of each word
///
/// Words are delimited by `-`, `_`, or whitespace
///
/// # Examples
/// ```ignore
/// capitalize_words("hello world")      // -> "Hello World"
/// capitalize_words("hello-world-test") // -> "Hello-World-Test"
/// capitalize_words("HELLO")            // -> "Hello"
/// ```
fn capitalize_words(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at_word_start = true;

    for c in text.chars() {
        if c == '-' || c == '_' || c.is_whitespace() {
            result.push(c);
            at_word_start = true;
        } else if at_word_start {
            result.extend(c.to_uppercase());
            at_word_start = false;
        } else {
            result.extend(c.to_lowercase());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // Default separator and case for tests
    const SEP_UNDERSCORE: char = '_';
    const SEP_DASH: char = '-';
    const CASE: SlugCase = SlugCase::Preserve;

    fn assert_sanitize_cases(sep: char, cases: &[(&str, &str)]) {
        for (input, expected) in cases {
            assert_eq!(sanitize(input, sep), *expected, "{input:?}");
        }
    }

    fn assert_transform_path_cases(
        sep: char,
        case: &SlugCase,
        to_ascii: bool,
        cases: &[(&str, &str)],
    ) {
        for (input, expected) in cases {
            let result = transform_path_components(Path::new(input), sep, case, to_ascii);
            assert_eq!(result, PathBuf::from(expected), "{input:?}");
        }
    }

    fn assert_slugify_full_cases(sep: char, cases: &[(&str, &str)]) {
        for (input, expected) in cases {
            assert_eq!(slugify_full(input, sep), *expected, "{input:?}");
        }
    }

    fn assert_apply_case_cases(case: &SlugCase, cases: &[(&str, &str)]) {
        for (input, expected) in cases {
            assert_eq!(apply_case(input, case), *expected, "{input:?}");
        }
    }

    // ========================================================================
    // sanitize() tests
    // ========================================================================

    #[test]
    fn test_sanitize_core_cases() {
        assert_sanitize_cases(
            SEP_UNDERSCORE,
            &[
                ("Hello<World>", "Hello_World"),
                ("a<b>c:d|e?f*g#h\\i(j)k[l]m", "a_b_c_d_e_f_g_h_i_j_k_l_m"),
                ("Hello World", "Hello_World"),
                ("Hello\tWorld\nTest", "Hello_World_Test"),
                ("  Hello World  ", "Hello_World"),
                (
                    "  Hello (World) [Test]: #anchor?  ",
                    "Hello_World_Test_anchor",
                ),
                ("", ""),
                ("<>:?*#", ""),
                ("My Article (2024) - Part #1", "My_Article_2024_-_Part_1"),
            ],
        );
    }

    // ========================================================================
    // Consecutive separator tests
    // ========================================================================

    #[test]
    fn test_sanitize_consecutive_separators() {
        // Consecutive forbidden chars and spaces should be collapsed into single separator
        assert_eq!(sanitize("A:   B", SEP_DASH), "A-B");
        assert_eq!(sanitize("A::::  ::: ::B", SEP_DASH), "A-B");
        assert_eq!(sanitize("Hello:::World", SEP_DASH), "Hello-World");
        assert_eq!(sanitize("a   b", SEP_DASH), "a-b");
        assert_eq!(sanitize("a<><><>b", SEP_DASH), "a-b");
        assert_eq!(sanitize("test::: :::test", SEP_UNDERSCORE), "test_test");
        assert_eq!(sanitize("A[[[B]]]C", SEP_DASH), "A-B-C");
        assert_eq!(sanitize("a((((b))))c", SEP_DASH), "a-b-c");
    }

    #[test]
    fn test_collapse_consecutive_separators() {
        assert_eq!(
            collapse_consecutive_separators("a--b--c", SEP_DASH),
            "a-b-c"
        );
        assert_eq!(collapse_consecutive_separators("--abc--", SEP_DASH), "abc");
        assert_eq!(collapse_consecutive_separators("------", SEP_DASH), "");
        assert_eq!(collapse_consecutive_separators("a-b-c", SEP_DASH), "a-b-c");
    }

    // ========================================================================
    // Unicode tests (SlugMode::Safe behavior)
    // ========================================================================

    #[test]
    fn test_sanitize_unicode_cases() {
        assert_sanitize_cases(
            SEP_UNDERSCORE,
            &[
                ("Café", "Café"),
                ("München", "München"),
                ("Über", "Über"),
                ("Café#World", "Café_World"),
                ("Über(Mich)", "Über_Mich"),
                ("Ich[Ich]", "Ich_Ich"),
                ("Start：End", "Start：End"),
                ("Start:End", "Start_End"),
                ("Café World", "Café_World"),
                ("  Über Mich  ", "Über_Mich"),
                ("こんにちは", "こんにちは"),
                ("日本語#テスト", "日本語_テスト"),
                ("안녕하세요", "안녕하세요"),
                ("한글 테스트", "한글_테스트"),
                ("Привет", "Привет"),
                ("Москва#Россия", "Москва_Россия"),
                ("señor", "señor"),
                ("Hello Café", "Hello_Café"),
                ("2024år", "2024år"),
                ("Hello 🎉", "Hello_🎉"),
                ("测试 🚀 emoji", "测试_🚀_emoji"),
            ],
        );
    }

    // ========================================================================
    // transform_path_components() tests
    // ========================================================================

    #[test]
    fn test_transform_path_safe_cases() {
        assert_transform_path_cases(
            SEP_UNDERSCORE,
            &CASE,
            false,
            &[
                ("content/posts/hello-world", "content/posts/hello-world"),
                ("content/posts/hello<world>", "content/posts/hello_world"),
                (
                    "content/my posts/hello world",
                    "content/my_posts/hello_world",
                ),
                ("content/Artikel/Café", "content/Artikel/Café"),
                (
                    "content/Artikel#1/Café[World]",
                    "content/Artikel_1/Café_World",
                ),
                ("posts/2024年/第一篇 文章", "posts/2024年/第一篇_文章"),
                ("ブログ/記事/こんにちは", "ブログ/記事/こんにちは"),
            ],
        );
    }

    #[test]
    fn test_transform_path_separator_and_case_modes() {
        assert_transform_path_cases(
            SEP_DASH,
            &CASE,
            false,
            &[(
                "content/my posts/hello world",
                "content/my-posts/hello-world",
            )],
        );
        assert_transform_path_cases(
            SEP_DASH,
            &SlugCase::Preserve,
            true,
            &[("content/Artikel/Café", "content/Artikel/Cafe")],
        );
        assert_transform_path_cases(
            SEP_DASH,
            &SlugCase::Lower,
            true,
            &[("content/Artikel/Café", "content/artikel/cafe")],
        );
        assert_transform_path_cases(
            SEP_DASH,
            &SlugCase::Lower,
            false,
            &[("Content/Posts/Hello World", "content/posts/hello-world")],
        );
        assert_transform_path_cases(
            SEP_DASH,
            &SlugCase::Upper,
            false,
            &[("content/posts/hello world", "CONTENT/POSTS/HELLO-WORLD")],
        );
        assert_transform_path_cases(
            SEP_DASH,
            &SlugCase::Capitalize,
            false,
            &[("content/posts/hello world", "Content/Posts/Hello-World")],
        );
    }

    // ========================================================================
    // slugify_full() tests (SlugMode::Full)
    // ========================================================================

    #[test]
    fn test_slugify_full_cases() {
        assert_slugify_full_cases(
            SEP_DASH,
            &[
                ("Hello World", "hello-world"),
                ("München", "munchen"),
                ("Åland", "aland"),
                ("café", "cafe"),
                ("über", "uber"),
                ("naïve", "naive"),
                ("Hello München", "hello-munchen"),
                ("2024år", "2024ar"),
            ],
        );
        assert_slugify_full_cases(SEP_UNDERSCORE, &[("Hello World", "hello_world")]);
    }

    // ========================================================================
    // Case transformation tests
    // ========================================================================

    #[test]
    fn test_apply_case_modes() {
        assert_apply_case_cases(
            &SlugCase::Lower,
            &[("Hello World", "hello world"), ("HELLO", "hello")],
        );
        assert_apply_case_cases(
            &SlugCase::Upper,
            &[("Hello World", "HELLO WORLD"), ("hello", "HELLO")],
        );
        assert_apply_case_cases(
            &SlugCase::Capitalize,
            &[
                ("hello world", "Hello World"),
                ("hello-world", "Hello-World"),
                ("hello_world", "Hello_World"),
                ("HELLO WORLD", "Hello World"),
            ],
        );
        assert_apply_case_cases(
            &SlugCase::Preserve,
            &[("Hello World", "Hello World"), ("hElLo", "hElLo")],
        );
    }

    #[test]
    fn test_capitalize_words() {
        assert_eq!(capitalize_words("hello world"), "Hello World");
        assert_eq!(capitalize_words("hello-world-test"), "Hello-World-Test");
        assert_eq!(capitalize_words("hello_world_test"), "Hello_World_Test");
        assert_eq!(capitalize_words("HELLO"), "Hello");
        assert_eq!(capitalize_words(""), "");
    }

    // ========================================================================
    // FORBIDDEN_CHARS constant tests
    // ========================================================================

    #[test]
    fn test_forbidden_chars_constant() {
        // Verify all expected forbidden characters are present
        let expected = [
            '<', '>', ':', '|', '?', '*', '#', '\\', '(', ')', '[', ']', '\t', '\r', '\n',
        ];
        for c in &expected {
            assert!(
                FORBIDDEN_CHARS.contains(c),
                "Missing forbidden char: {:?}",
                c
            );
        }
    }

    // ========================================================================
    // Integration tests with SlugConfig
    // ========================================================================

    fn make_slug_config(path_mode: &str, fragment_mode: &str, case: &str, sep: char) -> SlugConfig {
        let sep_str = if sep == '-' { "dash" } else { "underscore" };
        let toml = format!(
            r#"
            path = "{}"
            fragment = "{}"
            case = "{}"
            separator = "{}"
            "#,
            path_mode, fragment_mode, case, sep_str
        );
        toml::from_str(&toml).unwrap()
    }

    #[test]
    fn test_slugify_fragment_modes() {
        // Full mode
        let config = make_slug_config("safe", "full", "lower", SEP_DASH);
        assert_eq!(slugify_fragment("Hello World", &config), "hello-world");
        assert_eq!(slugify_fragment("München", &config), "munchen");

        // Safe mode
        let config = make_slug_config("safe", "safe", "preserve", SEP_UNDERSCORE);
        assert_eq!(slugify_fragment("Hello World", &config), "Hello_World");
        assert_eq!(slugify_fragment("München", &config), "München");

        // Ascii mode
        let config = make_slug_config("safe", "ascii", "lower", SEP_DASH);
        assert_eq!(slugify_fragment("Hello World", &config), "hello-world");
        assert_eq!(slugify_fragment("München", &config), "munchen");

        // No mode
        let config = make_slug_config("safe", "no", "preserve", SEP_DASH);
        assert_eq!(slugify_fragment("Hello World", &config), "Hello World");
    }

    #[test]
    fn test_slugify_path_modes() {
        // Full mode
        let config = make_slug_config("full", "safe", "lower", SEP_DASH);
        assert_eq!(
            slugify_path("content/My Posts/Hello", &config),
            PathBuf::from("content/my-posts/hello")
        );

        // Safe mode
        let config = make_slug_config("safe", "safe", "preserve", SEP_UNDERSCORE);
        assert_eq!(
            slugify_path("content/My Posts/Hello", &config),
            PathBuf::from("content/My_Posts/Hello")
        );

        // Ascii mode
        let config = make_slug_config("ascii", "safe", "lower", SEP_DASH);
        assert_eq!(
            slugify_path("content/My Posts/München", &config),
            PathBuf::from("content/my-posts/munchen")
        );

        // No mode
        let config = make_slug_config("no", "safe", "preserve", SEP_DASH);
        assert_eq!(
            slugify_path("content/My Posts/Hello", &config),
            PathBuf::from("content/My Posts/Hello")
        );
    }

    #[test]
    fn test_slugify_path_full_mode_preserves_structure() {
        let config = make_slug_config("full", "safe", "lower", SEP_DASH);

        // Test 1: Unicode paths - each component slugified separately
        assert_eq!(
            slugify_path("posts/北京/天安门", &config),
            PathBuf::from("posts/bei-jing/tian-an-men")
        );

        // Test 2: Deeply nested paths
        assert_eq!(
            slugify_path("a/b/c/d/e", &config),
            PathBuf::from("a/b/c/d/e")
        );

        // Test 3: Mixed case and spaces in multiple components
        assert_eq!(
            slugify_path("Blog Posts/2024/Hello World", &config),
            PathBuf::from("blog-posts/2024/hello-world")
        );

        // Test 4: Special characters in path components (note: + is preserved)
        assert_eq!(
            slugify_path("posts/C++ Guide/Part #1", &config),
            PathBuf::from("posts/c++-guide/part-1")
        );

        // Test 5: Single component (no path separators)
        assert_eq!(
            slugify_path("Hello World", &config),
            PathBuf::from("hello-world")
        );
    }
}
