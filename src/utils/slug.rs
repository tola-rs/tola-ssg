//! URL slugification and path sanitization utilities.
//!
//! This module provides functions to convert text and file paths into URL-safe formats.
//! It supports multiple slug modes and case transformations.
//!
//! # Slug Modes
//!
//! | Mode | Unicode | Forbidden Chars | Case | Example |
//! |------|---------|-----------------|------|---------|
//! | `On` | → ASCII | → separator | lowercase | `"你好 World"` → `"ni-hao-world"` |
//! | `Safe` | preserved | → separator | configurable | `"你好 World"` → `"你好-World"` |
//! | `Ascii` | → ASCII | → separator | configurable | `"你好 World"` → `"Ni-Hao-World"` |
//! | `No` | preserved | preserved | preserved | `"你好 World"` → `"你好 World"` |
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
//! sanitize_text("第一章:开始", '-') // → "第一章-开始"
//! sanitize_text("你::::好", '-')    // → "你-好" (consecutive collapsed)
//!
//! // Full slugify: converts to ASCII lowercase
//! slugify_on("你好世界", '-')       // → "ni-hao-shi-jie"
//! ```

use crate::config::{SiteConfig, SlugCase, SlugMode};
use std::path::{Path, PathBuf};

/// Characters that are unsafe for URLs and file paths.
///
/// These characters are replaced with the configured separator.
/// Consecutive occurrences are collapsed into a single separator.
pub const FORBIDDEN_CHARS: &[char] = &[
    '<', '>', ':', '|', '?', '*', '#', '\\', '(', ')', '[', ']', '\t', '\r', '\n',
];

// ============================================================================
// Public API
// ============================================================================

/// Converts fragment text (e.g., heading anchors) to URL-safe format.
///
/// # Arguments
/// * `text` - The text to slugify
/// * `config` - Site configuration containing slug settings
///
/// # Example
/// ```ignore
/// // With SlugMode::Safe, separator='-', case=Lower
/// slugify_fragment("Hello World") // → "hello-world"
/// slugify_fragment("第一章:开始") // → "第一章-开始"
/// ```
pub fn slugify_fragment(text: &str, config: &'static SiteConfig) -> String {
    let slug = &config.build.slug;
    let sep = slug.separator.as_char();

    let result = match slug.fragment {
        SlugMode::No => return text.to_owned(),
        SlugMode::Full => slugify_full(text, sep),
        SlugMode::Safe => sanitize(text, sep),
        SlugMode::Ascii => sanitize(&deunicode::deunicode(text), sep),
    };

    apply_case(&result, &slug.case)
}

/// Converts a file path to URL-safe format.
///
/// Each path component is processed independently, preserving the directory structure.
///
/// # Arguments
/// * `path` - The path to slugify
/// * `config` - Site configuration containing slug settings
///
/// # Example
/// ```ignore
/// // With SlugMode::Safe, separator='-', case=Lower
/// slugify_path("content/My Posts/Hello World")
/// // → "content/my-posts/hello-world"
/// ```
pub fn slugify_path(path: impl AsRef<Path>, config: &'static SiteConfig) -> PathBuf {
    let slug = &config.build.slug;
    let sep = slug.separator.as_char();

    match slug.path {
        SlugMode::No => path.as_ref().to_path_buf(),
        SlugMode::Full => slugify_full(&path.as_ref().to_string_lossy(), sep).into(),
        SlugMode::Safe => transform_path_components(path.as_ref(), sep, &slug.case, false),
        SlugMode::Ascii => transform_path_components(path.as_ref(), sep, &slug.case, true),
    }
}

/// Removes forbidden characters from a string.
///
/// This is a utility function for other modules (like `meta.rs`) that need to
/// sanitize strings using the same rules as the slug module, but without
/// applying full slugification logic (separators, case, etc.).
pub fn remove_forbidden_chars(text: &str) -> String {
    text.chars()
        .filter(|c| !FORBIDDEN_CHARS.contains(c))
        .collect()
}

// ============================================================================
// Core Transformation Functions
// ============================================================================

/// Full slugification: Unicode → ASCII, lowercase, separator-delimited.
///
/// This is the most aggressive transformation, suitable for URL slugs.
/// Always produces lowercase output regardless of case settings.
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
/// slugify_full("Hello World", '-')  // → "hello-world"
/// slugify_full("你好世界", '-')      // → "ni-hao-shi-jie"
/// slugify_full("Café Naïve", '-')   // → "cafe-naive"
/// slugify_full("a:::b", '-')        // → "a-b"
/// ```
fn slugify_full(text: &str, sep: char) -> String {
    let ascii = deunicode::deunicode(text);
    let replaced = replace_special_chars(&ascii.to_lowercase(), sep);
    collapse_consecutive_separators(&replaced, sep)
}

/// Sanitizes text by replacing forbidden characters with separator.
///
/// Preserves Unicode characters while making the text URL-safe.
///
/// # Processing Steps
/// 1. Trim leading/trailing whitespace
/// 2. Replace forbidden chars and whitespace with separator
/// 3. Collapse consecutive separators
/// 4. Trim leading/trailing separators
///
/// # Examples
/// ```ignore
/// sanitize("Hello World", '-')   // → "Hello-World"
/// sanitize("你好#世界", '-')      // → "你好-世界"
/// sanitize("a:::b   c", '-')     // → "a-b-c"
/// sanitize("第一章:开始", '-')   // → "第一章-开始"
/// ```
fn sanitize(text: &str, sep: char) -> String {
    let replaced = replace_special_chars(text.trim(), sep);
    collapse_consecutive_separators(&replaced, sep)
}

/// Transforms each component of a path independently.
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
            apply_case(&sanitized, case)
        })
        .collect()
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Replaces forbidden characters and whitespace with the separator.
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

/// Collapses consecutive separators into one and trims leading/trailing separators.
///
/// # Examples
/// ```ignore
/// collapse_consecutive_separators("a--b--c", '-') // → "a-b-c"
/// collapse_consecutive_separators("--abc--", '-') // → "abc"
/// collapse_consecutive_separators("------", '-')  // → ""
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

/// Applies case transformation to text.
///
/// # Case Modes
/// - `Lower`: all lowercase
/// - `Upper`: ALL UPPERCASE
/// - `Capitalize`: Title Case (Each Word Capitalized)
/// - `Preserve`: no change
fn apply_case(text: &str, case: &SlugCase) -> String {
    match case {
        SlugCase::Lower => text.to_lowercase(),
        SlugCase::Upper => text.to_uppercase(),
        SlugCase::Capitalize => capitalize_words(text),
        SlugCase::Preserve => text.to_owned(),
    }
}

/// Capitalizes the first letter of each word.
///
/// Words are delimited by `-`, `_`, or whitespace.
///
/// # Examples
/// ```ignore
/// capitalize_words("hello world")      // → "Hello World"
/// capitalize_words("hello-world-test") // → "Hello-World-Test"
/// capitalize_words("HELLO")            // → "Hello"
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

    // ========================================================================
    // sanitize() tests
    // ========================================================================

    #[test]
    fn test_sanitize_replaces_forbidden_chars() {
        assert_eq!(sanitize("Hello<World>", SEP_UNDERSCORE), "Hello_World");
    }

    #[test]
    fn test_sanitize_replaces_all_forbidden_chars() {
        // All forbidden chars replaced, consecutive separators collapsed
        assert_eq!(sanitize("a<b>c:d|e?f*g#h\\i(j)k[l]m", SEP_UNDERSCORE), "a_b_c_d_e_f_g_h_i_j_k_l_m");
    }

    #[test]
    fn test_sanitize_replaces_whitespace() {
        assert_eq!(sanitize("Hello World", SEP_UNDERSCORE), "Hello_World");
    }

    #[test]
    fn test_sanitize_replaces_various_whitespace() {
        // \t and \n are forbidden chars, replaced with separator
        assert_eq!(sanitize("Hello\tWorld\nTest", SEP_UNDERSCORE), "Hello_World_Test");
    }

    #[test]
    fn test_sanitize_trims() {
        assert_eq!(sanitize("  Hello World  ", SEP_UNDERSCORE), "Hello_World");
    }

    #[test]
    fn test_sanitize_preserves_unicode() {
        assert_eq!(sanitize("你好世界", SEP_UNDERSCORE), "你好世界");
    }

    #[test]
    fn test_sanitize_complex_input() {
        // Forbidden chars replaced, consecutive separators collapsed
        assert_eq!(sanitize("  Hello (World) [Test]: #anchor?  ", SEP_UNDERSCORE), "Hello_World_Test_anchor");
    }

    #[test]
    fn test_sanitize_empty_string() {
        assert_eq!(sanitize("", SEP_UNDERSCORE), "");
    }

    #[test]
    fn test_sanitize_only_forbidden_chars() {
        // All forbidden chars collapse to empty string
        assert_eq!(sanitize("<>:?*#", SEP_UNDERSCORE), "");
    }

    #[test]
    fn test_sanitize_mixed_content() {
        // () and # replaced with separator, consecutive collapsed
        assert_eq!(sanitize("My Article (2024) - Part #1", SEP_UNDERSCORE), "My_Article_2024_-_Part_1");
    }

    // ========================================================================
    // Consecutive separator tests
    // ========================================================================

    #[test]
    fn test_sanitize_consecutive_separators() {
        // Consecutive forbidden chars and spaces should be collapsed into single separator
        assert_eq!(sanitize("你:   好", SEP_DASH), "你-好");
        assert_eq!(sanitize("你::::  ::: ::好", SEP_DASH), "你-好");
        assert_eq!(sanitize("Hello:::World", SEP_DASH), "Hello-World");
        assert_eq!(sanitize("a   b", SEP_DASH), "a-b");
        assert_eq!(sanitize("a<><><>b", SEP_DASH), "a-b");
        assert_eq!(sanitize("test::: :::test", SEP_UNDERSCORE), "test_test");
        assert_eq!(sanitize("你[[[好]]]世界", SEP_DASH), "你-好-世界");
        assert_eq!(sanitize("a((((b))))c", SEP_DASH), "a-b-c");
    }

    #[test]
    fn test_collapse_consecutive_separators() {
        assert_eq!(collapse_consecutive_separators("a--b--c", SEP_DASH), "a-b-c");
        assert_eq!(collapse_consecutive_separators("--abc--", SEP_DASH), "abc");
        assert_eq!(collapse_consecutive_separators("------", SEP_DASH), "");
        assert_eq!(collapse_consecutive_separators("a-b-c", SEP_DASH), "a-b-c");
    }

    // ========================================================================
    // Unicode tests (SlugMode::Safe behavior)
    // ========================================================================

    #[test]
    fn test_sanitize_chinese() {
        assert_eq!(sanitize("你好", SEP_UNDERSCORE), "你好");
        assert_eq!(sanitize("你好世界", SEP_UNDERSCORE), "你好世界");
        assert_eq!(sanitize("关于我", SEP_UNDERSCORE), "关于我");
    }

    #[test]
    fn test_sanitize_chinese_with_forbidden() {
        // Forbidden chars are replaced with separator
        assert_eq!(sanitize("你好#世界", SEP_UNDERSCORE), "你好_世界");
        assert_eq!(sanitize("关于(我)", SEP_UNDERSCORE), "关于_我");
        assert_eq!(sanitize("我[我]", SEP_UNDERSCORE), "我_我");
        assert_eq!(sanitize("第一章：开始", SEP_UNDERSCORE), "第一章：开始"); // Chinese colon ：is NOT forbidden
        assert_eq!(sanitize("第一章:开始", SEP_UNDERSCORE), "第一章_开始"); // ASCII colon : IS forbidden
    }

    #[test]
    fn test_sanitize_chinese_with_spaces() {
        assert_eq!(sanitize("你好 世界", SEP_UNDERSCORE), "你好_世界");
        assert_eq!(sanitize("  关于 我  ", SEP_UNDERSCORE), "关于_我");
    }

    #[test]
    fn test_sanitize_japanese() {
        assert_eq!(sanitize("こんにちは", SEP_UNDERSCORE), "こんにちは");
        assert_eq!(sanitize("コンニチハ", SEP_UNDERSCORE), "コンニチハ");
        assert_eq!(sanitize("日本語#テスト", SEP_UNDERSCORE), "日本語_テスト");
    }

    #[test]
    fn test_sanitize_korean() {
        assert_eq!(sanitize("안녕하세요", SEP_UNDERSCORE), "안녕하세요");
        assert_eq!(sanitize("한글 테스트", SEP_UNDERSCORE), "한글_테스트");
    }

    #[test]
    fn test_sanitize_cyrillic() {
        assert_eq!(sanitize("Привет", SEP_UNDERSCORE), "Привет");
        assert_eq!(sanitize("Москва#Россия", SEP_UNDERSCORE), "Москва_Россия");
    }

    #[test]
    fn test_sanitize_european_accents() {
        assert_eq!(sanitize("café", SEP_UNDERSCORE), "café");
        assert_eq!(sanitize("naïve", SEP_UNDERSCORE), "naïve");
        assert_eq!(sanitize("über", SEP_UNDERSCORE), "über");
        assert_eq!(sanitize("señor", SEP_UNDERSCORE), "señor");
    }

    #[test]
    fn test_sanitize_mixed_unicode_ascii() {
        assert_eq!(sanitize("Hello 你好", SEP_UNDERSCORE), "Hello_你好");
        assert_eq!(sanitize("About 关于", SEP_UNDERSCORE), "About_关于");
        assert_eq!(sanitize("2024年总结", SEP_UNDERSCORE), "2024年总结");
        assert_eq!(sanitize("第1章", SEP_UNDERSCORE), "第1章");
    }

    #[test]
    fn test_sanitize_emoji() {
        assert_eq!(sanitize("Hello 🎉", SEP_UNDERSCORE), "Hello_🎉");
        assert_eq!(sanitize("测试 🚀 emoji", SEP_UNDERSCORE), "测试_🚀_emoji");
    }

    // ========================================================================
    // transform_path_components() tests
    // ========================================================================

    #[test]
    fn test_transform_path_simple() {
        let path = Path::new("content/posts/hello-world");
        let result = transform_path_components(path, SEP_UNDERSCORE, &CASE, false);
        assert_eq!(result, PathBuf::from("content/posts/hello-world"));
    }

    #[test]
    fn test_transform_path_with_forbidden_chars() {
        let path = Path::new("content/posts/hello<world>");
        let result = transform_path_components(path, SEP_UNDERSCORE, &CASE, false);
        assert_eq!(result, PathBuf::from("content/posts/hello_world"));
    }

    #[test]
    fn test_transform_path_with_spaces() {
        let path = Path::new("content/my posts/hello world");
        let result = transform_path_components(path, SEP_UNDERSCORE, &CASE, false);
        assert_eq!(result, PathBuf::from("content/my_posts/hello_world"));
    }

    #[test]
    fn test_transform_path_chinese() {
        let path = Path::new("content/文章/你好世界");
        let result = transform_path_components(path, SEP_UNDERSCORE, &CASE, false);
        assert_eq!(result, PathBuf::from("content/文章/你好世界"));
    }

    #[test]
    fn test_transform_path_chinese_with_forbidden() {
        let path = Path::new("content/文章#1/你好[世界]");
        let result = transform_path_components(path, SEP_UNDERSCORE, &CASE, false);
        assert_eq!(result, PathBuf::from("content/文章_1/你好_世界"));
    }

    #[test]
    fn test_transform_path_mixed_unicode() {
        let path = Path::new("posts/2024年/第一篇 文章");
        let result = transform_path_components(path, SEP_UNDERSCORE, &CASE, false);
        assert_eq!(result, PathBuf::from("posts/2024年/第一篇_文章"));
    }

    #[test]
    fn test_transform_path_japanese() {
        let path = Path::new("ブログ/記事/こんにちは");
        let result = transform_path_components(path, SEP_UNDERSCORE, &CASE, false);
        assert_eq!(result, PathBuf::from("ブログ/記事/こんにちは"));
    }

    #[test]
    fn test_transform_path_with_hyphen_separator() {
        let path = Path::new("content/my posts/hello world");
        let result = transform_path_components(path, SEP_DASH, &CASE, false);
        assert_eq!(result, PathBuf::from("content/my-posts/hello-world"));
    }

    #[test]
    fn test_transform_path_ascii_mode() {
        let path = Path::new("content/文章/你好世界");
        let result = transform_path_components(path, SEP_DASH, &SlugCase::Preserve, true);
        assert_eq!(result, PathBuf::from("content/Wen-Zhang/Ni-Hao-Shi-Jie"));
    }

    #[test]
    fn test_transform_path_ascii_with_case_lower() {
        let path = Path::new("content/文章/你好世界");
        let result = transform_path_components(path, SEP_DASH, &SlugCase::Lower, true);
        assert_eq!(result, PathBuf::from("content/wen-zhang/ni-hao-shi-jie"));
    }

    #[test]
    fn test_transform_path_with_case_lower() {
        let path = Path::new("Content/Posts/Hello World");
        let result = transform_path_components(path, SEP_DASH, &SlugCase::Lower, false);
        assert_eq!(result, PathBuf::from("content/posts/hello-world"));
    }

    #[test]
    fn test_transform_path_with_case_upper() {
        let path = Path::new("content/posts/hello world");
        let result = transform_path_components(path, SEP_DASH, &SlugCase::Upper, false);
        assert_eq!(result, PathBuf::from("CONTENT/POSTS/HELLO-WORLD"));
    }

    #[test]
    fn test_transform_path_with_case_capitalize() {
        let path = Path::new("content/posts/hello world");
        let result = transform_path_components(path, SEP_DASH, &SlugCase::Capitalize, false);
        assert_eq!(result, PathBuf::from("Content/Posts/Hello-World"));
    }

    // ========================================================================
    // slugify_full() tests (SlugMode::Full)
    // ========================================================================

    #[test]
    fn test_slugify_full_basic() {
        assert_eq!(slugify_full("Hello World", SEP_DASH), "hello-world");
        assert_eq!(slugify_full("Hello World", SEP_UNDERSCORE), "hello_world");
    }

    #[test]
    fn test_slugify_full_unicode_to_ascii() {
        // Chinese → Pinyin
        assert_eq!(slugify_full("你好", SEP_DASH), "ni-hao");
        assert_eq!(slugify_full("你好世界", SEP_DASH), "ni-hao-shi-jie");

        // European accents → ASCII
        assert_eq!(slugify_full("café", SEP_DASH), "cafe");
        assert_eq!(slugify_full("über", SEP_DASH), "uber");
        assert_eq!(slugify_full("naïve", SEP_DASH), "naive");
    }

    #[test]
    fn test_slugify_full_mixed() {
        assert_eq!(slugify_full("Hello 你好", SEP_DASH), "hello-ni-hao");
        // Note: 2024年 → "2024nian" (no space between number and transliteration)
        assert_eq!(slugify_full("2024年总结", SEP_DASH), "2024nian-zong-jie");
    }

    // ========================================================================
    // Case transformation tests
    // ========================================================================

    #[test]
    fn test_apply_case_lower() {
        assert_eq!(apply_case("Hello World", &SlugCase::Lower), "hello world");
        assert_eq!(apply_case("HELLO", &SlugCase::Lower), "hello");
    }

    #[test]
    fn test_apply_case_upper() {
        assert_eq!(apply_case("Hello World", &SlugCase::Upper), "HELLO WORLD");
        assert_eq!(apply_case("hello", &SlugCase::Upper), "HELLO");
    }

    #[test]
    fn test_apply_case_capitalize() {
        assert_eq!(apply_case("hello world", &SlugCase::Capitalize), "Hello World");
        assert_eq!(apply_case("hello-world", &SlugCase::Capitalize), "Hello-World");
        assert_eq!(apply_case("hello_world", &SlugCase::Capitalize), "Hello_World");
        assert_eq!(apply_case("HELLO WORLD", &SlugCase::Capitalize), "Hello World");
    }

    #[test]
    fn test_apply_case_preserve() {
        assert_eq!(apply_case("Hello World", &SlugCase::Preserve), "Hello World");
        assert_eq!(apply_case("hElLo", &SlugCase::Preserve), "hElLo");
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
        let expected = ['<', '>', ':', '|', '?', '*', '#', '\\', '(', ')', '[', ']', '\t', '\r', '\n'];
        for c in &expected {
            assert!(FORBIDDEN_CHARS.contains(c), "Missing forbidden char: {:?}", c);
        }
    }

    #[test]
    fn test_remove_forbidden_chars() {
        assert_eq!(remove_forbidden_chars("Hello<World>"), "HelloWorld");
        assert_eq!(remove_forbidden_chars("a<b>c:d|e?f*g#h\\i(j)k[l]m"), "abcdefghijklm");
        assert_eq!(remove_forbidden_chars("Hello World"), "Hello World");
        assert_eq!(remove_forbidden_chars("Hello\tWorld\nTest"), "HelloWorldTest");
    }

    // ========================================================================
    // Integration tests with SiteConfig
    // ========================================================================

    fn make_config(path_mode: &str, fragment_mode: &str, case: &str, sep: char) -> &'static SiteConfig {
        let sep_str = if sep == '-' { "dash" } else { "underscore" };
        let toml = format!(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.slug]
            path = "{}"
            fragment = "{}"
            case = "{}"
            separator = "{}"
            "#,
            path_mode, fragment_mode, case, sep_str
        );
        // Leak memory to get &'static SiteConfig for tests
        let config: SiteConfig = toml::from_str(&toml).unwrap();
        Box::leak(Box::new(config))
    }

    #[test]
    fn test_slugify_fragment_modes() {
        // Full mode
        let config = make_config("safe", "full", "lower", SEP_DASH);
        assert_eq!(slugify_fragment("Hello World", config), "hello-world");
        assert_eq!(slugify_fragment("你好", config), "ni-hao");

        // Safe mode
        let config = make_config("safe", "safe", "preserve", SEP_UNDERSCORE);
        assert_eq!(slugify_fragment("Hello World", config), "Hello_World");
        assert_eq!(slugify_fragment("你好", config), "你好");

        // Ascii mode
        let config = make_config("safe", "ascii", "lower", SEP_DASH);
        assert_eq!(slugify_fragment("Hello World", config), "hello-world");
        assert_eq!(slugify_fragment("你好", config), "ni-hao");

        // No mode
        let config = make_config("safe", "no", "preserve", SEP_DASH);
        assert_eq!(slugify_fragment("Hello World", config), "Hello World");
    }

    #[test]
    fn test_slugify_path_modes() {
        // Full mode
        let config = make_config("full", "safe", "lower", SEP_DASH);
        assert_eq!(slugify_path("content/My Posts/Hello", config), PathBuf::from("content/my-posts/hello"));

        // Safe mode
        let config = make_config("safe", "safe", "preserve", SEP_UNDERSCORE);
        assert_eq!(slugify_path("content/My Posts/Hello", config), PathBuf::from("content/My_Posts/Hello"));

        // Ascii mode
        let config = make_config("ascii", "safe", "lower", SEP_DASH);
        assert_eq!(slugify_path("content/My Posts/你好", config), PathBuf::from("content/my-posts/ni-hao"));

        // No mode
        let config = make_config("no", "safe", "preserve", SEP_DASH);
        assert_eq!(slugify_path("content/My Posts/Hello", config), PathBuf::from("content/My Posts/Hello"));
    }
}
