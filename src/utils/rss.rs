//! RSS feed generation.
//!
//! Parses post metadata and generates RSS/Atom feeds.

use crate::{
    config::SiteConfig,
    exec, log,
    utils::{build::collect_files, slug::content_paths},
};
use anyhow::{Context, Ok, Result, anyhow, bail};
use rayon::prelude::*;
use regex::Regex;
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder, validation::Validate};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::LazyLock};

// ============================================================================
// Constants
// ============================================================================

/// Tag name for querying typst metadata
const META_TAG_NAME: &str = "<tola-meta>";

// ============================================================================
// Date/Time Types
// ============================================================================

/// UTC datetime without timezone complexity
#[derive(Debug, Clone, Copy)]
pub struct DateTimeUtc {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[allow(dead_code)]
impl DateTimeUtc {
    pub const fn new(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    pub const fn from_ymd(year: u16, month: u8, day: u8) -> Self {
        Self::new(year, month, day, 0, 0, 0)
    }

    /// Parse from "YYYY-MM-DD" or "YYYY-MM-DDTHH:MM:SSZ" format
    pub fn parse(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();

        // Minimum: "YYYY-MM-DD" (10 chars)
        if bytes.len() < 10 {
            return None;
        }

        // Parse date part
        let year = parse_u16(&bytes[0..4])?;
        if bytes[4] != b'-' {
            return None;
        }
        let month = parse_u8(&bytes[5..7])?;
        if bytes[7] != b'-' {
            return None;
        }
        let day = parse_u8(&bytes[8..10])?;

        // Check for time part (RFC3339)
        let (hour, minute, second) = if bytes.len() >= 20 && bytes[10] == b'T' && bytes[19] == b'Z'
        {
            if bytes[13] != b':' || bytes[16] != b':' {
                return None;
            }
            (
                parse_u8(&bytes[11..13])?,
                parse_u8(&bytes[14..16])?,
                parse_u8(&bytes[17..19])?,
            )
        } else if bytes.len() == 10 {
            (0, 0, 0)
        } else {
            return None;
        };

        let dt = Self::new(year, month, day, hour, minute, second);
        dt.validate().ok()?;
        Some(dt)
    }

    pub fn validate(&self) -> Result<()> {
        let Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        } = *self;

        if !(1..=12).contains(&month) {
            bail!("month is invalid: {month}");
        }

        let max_days = Self::days_in_month(year, month);
        if day == 0 || day > max_days {
            bail!("day is invalid: {day}");
        }
        if hour > 23 {
            bail!("hour is invalid: {hour}");
        }
        if minute > 59 {
            bail!("minute is invalid: {minute}");
        }
        if second > 59 {
            bail!("second is invalid: {second}");
        }

        Ok(())
    }

    #[inline]
    fn is_leap_year(year: u16) -> bool {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    }

    #[inline]
    fn days_in_month(year: u16, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if Self::is_leap_year(year) => 29,
            2 => 28,
            _ => 0,
        }
    }

    pub fn to_rfc2822(self) -> String {
        const WEEKDAYS: [&str; 7] = ["Sat", "Sun", "Mon", "Tue", "Wed", "Thu", "Fri"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];

        // Zeller's congruence for weekday calculation
        let weekday = self.weekday_index();

        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAYS[weekday],
            self.day,
            MONTHS[(self.month - 1) as usize],
            self.year,
            self.hour,
            self.minute,
            self.second
        )
    }

    #[inline]
    fn weekday_index(&self) -> usize {
        let (y, m) = if self.month < 3 {
            (self.year as i32 - 1, self.month as i32 + 12)
        } else {
            (self.year as i32, self.month as i32)
        };
        let d = self.day as i32;
        ((d + (13 * (m + 1)) / 5 + y + y / 4 - y / 100 + y / 400) % 7) as usize
    }
}

/// Parse 2-digit ASCII number
#[inline]
fn parse_u8(bytes: &[u8]) -> Option<u8> {
    if bytes.len() != 2 {
        return None;
    }
    let d1 = bytes[0].wrapping_sub(b'0');
    let d2 = bytes[1].wrapping_sub(b'0');
    if d1 > 9 || d2 > 9 {
        return None;
    }
    Some(d1 * 10 + d2)
}

/// Parse 4-digit ASCII number
#[inline]
fn parse_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    let mut result = 0u16;
    for &b in bytes {
        let d = b.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        result = result * 10 + d as u16;
    }
    Some(result)
}

// ============================================================================
// RSS Feed Types
// ============================================================================

/// RSS feed builder
pub struct RssFeed {
    title: String,
    description: String,
    base_url: String,
    language: String,
    posts: Vec<PostMeta>,
}

/// Metadata extracted from a post
#[derive(Debug, Default, Serialize, Deserialize)]
struct PostMeta {
    title: Option<String>,
    summary: Option<String>,
    date: Option<String>,
    #[allow(dead_code)]
    update: Option<String>,
    #[serde(default)]
    link: Option<String>,
    author: Option<String>,
}

impl PostMeta {
    /// Convert to RSS item, returns None if required fields are missing
    fn into_rss_item(self) -> Option<rss::Item> {
        let title = self.title?;
        let link = self.link.clone()?;
        let pub_date = DateTimeUtc::parse(self.date.as_deref()?).map(|dt| dt.to_rfc2822())?;

        Some(
            ItemBuilder::default()
                .title(title)
                .link(self.link)
                .guid(GuidBuilder::default().permalink(true).value(link).build())
                .description(self.summary)
                .pub_date(pub_date)
                .author(self.author)
                .build(),
        )
    }
}

// ============================================================================
// Typst Element Parsing
// ============================================================================

/// Represents parsed Typst content elements
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "func", rename_all = "lowercase")]
enum TypstElement {
    Space,
    Linebreak,
    Text {
        text: String,
    },
    Strike {
        text: String,
    },
    Link {
        dest: String,
        body: Box<TypstElement>,
    },
    Sequence {
        children: Vec<TypstElement>,
    },
    #[serde(other)]
    Unknown,
}

impl TypstElement {
    /// Convert Typst element to HTML string
    fn to_html(&self, base_url: &str) -> String {
        match self {
            Self::Space => " ".into(),
            Self::Linebreak => "<br/>".into(),
            Self::Text { text } => html_escape(text),
            Self::Strike { text } => format!("<s>{}</s>", html_escape(text)),
            Self::Link { dest, body } => {
                let href = normalize_link(dest, base_url);
                format!("<a href=\"{}\">{}</a>", href, body.to_html(base_url))
            }
            Self::Sequence { children } => {
                let mut result = String::new();
                for child in children {
                    result.push_str(&child.to_html(base_url));
                }
                result
            }
            Self::Unknown => String::new(),
        }
    }
}

/// Escape HTML special characters
#[inline]
fn html_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '&' => result.push_str("&amp;"),
            '"' => result.push_str("&quot;"),
            _ => result.push(c),
        }
    }
    result
}

/// Normalize relative links to absolute URLs
#[inline]
fn normalize_link(dest: &str, base_url: &str) -> String {
    if dest.starts_with(['.', '/']) {
        let path = dest.trim_start_matches(['.', '/']);
        format!("{}/{}", base_url.trim_end_matches('/'), path)
    } else {
        dest.to_string()
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Build RSS feed if enabled in config.
///
/// This checks `config.build.rss.enable` before generating.
/// The caller (`build_all` in `main.rs`) may also skip calling this entirely
/// in serve mode to speed up local preview.
pub fn build_rss(config: &'static SiteConfig) -> Result<()> {
    if config.build.rss.enable {
        RssFeed::build(config)?.write(config)?;
    }
    Ok(())
}

/// Generate GUID URL for a content file.
///
/// Converts a `.typ` content path to its corresponding public URL.
/// Example: `content/posts/hello.typ` → `https://example.com/posts/hello/index.html`
pub fn get_guid_from_content_path(
    content_path: &Path,
    config: &'static SiteConfig,
) -> Result<String> {
    let base_url = config.base.url.as_deref().unwrap_or_default();
    let paths = content_paths(content_path, config)?;

    // Strip output dir prefix to get relative path for URL
    let html_relative = paths
        .html
        .strip_prefix(&config.build.output)
        .unwrap_or(&paths.html);

    // URL-encode path components but preserve slashes
    let encoded = urlencoding::encode(html_relative.to_str().unwrap_or_default());
    let encoded = encoded.replace("%2F", "/");

    Ok(format!("{}/{}", base_url.trim_end_matches('/'), encoded))
}

// ============================================================================
// RssFeed Implementation
// ============================================================================

impl RssFeed {
    /// Build RSS feed by collecting and parsing all posts
    pub fn build(config: &'static SiteConfig) -> Result<Self> {
        log!(true; "rss"; "generating rss feed started");

        let posts_paths = collect_files(&config.build.content, |path| {
            path.extension().is_some_and(|ext| ext == "typ")
        });

        let posts: Vec<PostMeta> = posts_paths
            .par_iter()
            .map(|path| query_post_meta(path, config))
            .collect::<Result<_>>()?;

        Ok(Self {
            title: config.base.title.clone(),
            description: config.base.description.clone(),
            base_url: config.base.url.clone().unwrap_or_default(),
            language: config.base.language.clone(),
            posts,
        })
    }

    /// Generate RSS XML string
    fn into_xml(self) -> Result<String> {
        let items: Vec<_> = self
            .posts
            .into_iter()
            .filter_map(PostMeta::into_rss_item)
            .collect();

        let channel = ChannelBuilder::default()
            .title(self.title)
            .link(self.base_url)
            .description(self.description)
            .language(self.language)
            .generator("tola-ssg".to_string())
            .items(items)
            .build();

        channel
            .validate()
            .map_err(|e| anyhow!("RSS validation failed: {e}"))?;
        Ok(channel.to_string())
    }

    /// Write RSS feed to file
    pub fn write(self, config: &'static SiteConfig) -> Result<()> {
        let xml = self.into_xml()?;
        let rss_path = &config.build.rss.path;

        if let Some(parent) = rss_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(rss_path, xml)?;

        log!(true; "rss"; "rss feed written successfully");
        Ok(())
    }
}

// ============================================================================
// Metadata Extraction
// ============================================================================

/// Query metadata from a Typst post file
fn query_post_meta(post_path: &Path, config: &'static SiteConfig) -> Result<PostMeta> {
    let root = config.get_root();
    let guid = get_guid_from_content_path(post_path, config)?;

    let output = exec!(
        &config.build.typst.command;
        "query", "--features", "html", "--format", "json",
        "--font-path", root, "--root", root,
        post_path,
        META_TAG_NAME, "--field", "value", "--one"
    )
    .with_context(|| {
        format!(
            "Failed to query metadata for post: {}\nEnsure tag name \"{}\" is correct",
            post_path.display(),
            META_TAG_NAME
        )
    })?;

    let json_str = std::str::from_utf8(&output.stdout)?;
    parse_post_meta(guid, json_str, config)
}

/// Parse post metadata from JSON string
fn parse_post_meta(guid: String, json_str: &str, config: &'static SiteConfig) -> Result<PostMeta> {
    let json: serde_json::Value = serde_json::from_str(json_str)
        .with_context(|| format!("Failed to parse post metadata JSON:\n{json_str}"))?;

    let get_string = |key: &str| json.get(key).and_then(|v| v.as_str()).map(String::from);

    // Parse summary from Typst element
    let base_url = config.base.url.as_deref().unwrap_or_default();
    let summary = get_string("summary")
        .and_then(|s| parse_typst_element(&s).ok())
        .map(|elem| elem.to_html(base_url));

    // Process author field
    let author = get_string("author");
    let author = normalize_rss_author(author.as_ref(), config);

    Ok(PostMeta {
        title: get_string("title"),
        summary,
        date: get_string("date"),
        update: get_string("update"),
        link: Some(guid),
        author,
    })
}

/// Parse Typst element from JSON string
fn parse_typst_element(content: &str) -> Result<TypstElement> {
    serde_json::from_str(content).map_err(Into::into)
}

/// Normalize author field to RSS format: "email@example.com (Name)"
///
/// Priority:
/// 1. Post meta author if already in valid format
/// 2. Site config author if in valid format
/// 3. Combine site config email and author
fn normalize_rss_author(author: Option<&String>, config: &'static SiteConfig) -> Option<String> {
    static RE_VALID_AUTHOR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\s*\([^)]+\)$").unwrap()
    });

    let author = author?;

    // Check if post author is already valid
    if RE_VALID_AUTHOR.is_match(author) {
        return Some(author.clone());
    }

    // Try site config author
    let site_author = &config.base.author;
    if RE_VALID_AUTHOR.is_match(site_author) {
        return Some(site_author.clone());
    }

    // Combine email and author name
    Some(format!("{} ({})", config.base.email, site_author))
}

#[test]
fn test_parse_element_from_typst_sequence() {
    let json_str = r#"
    {
        "func": "sequence",
        "children": [
            { "func": "space" },
            { "func": "text", "text": "This is a simple, fluent, and flexible typing scheme" },
            { "func": "space" },
            { "func": "linebreak" },
            { "func": "space" },
            { "func": "link", "dest": "https://example.com", "body": { "func": "text", "text": "Learn more" } },
            { "func": "text", "text": "Suitable for those who want to improve typing speed without too much effort" },
            { "func": "space" },
            { "func": "unknown_func" }
        ]
    }
    "#;

    let result = parse_typst_element(json_str).unwrap();
    assert_eq!(
        result,
        TypstElement::Sequence {
            children: vec![
                TypstElement::Space,
                TypstElement::Text {
                    text: "This is a simple, fluent, and flexible typing scheme".to_string()
                },
                TypstElement::Space,
                TypstElement::Linebreak,
                TypstElement::Space,
                TypstElement::Link {
                    dest: "https://example.com".to_string(),
                    body: Box::new(TypstElement::Text {
                        text: "Learn more".to_string()
                    }),
                },
                TypstElement::Text {
                    text: "Suitable for those who want to improve typing speed without too much effort"
                        .to_string()
                },
                TypstElement::Space,
                TypstElement::Unknown,
            ]
        }
    );
}

#[test]
fn test_datetime_utc_new() {
    let dt = DateTimeUtc::new(2024, 6, 15, 14, 30, 45);
    assert_eq!(dt.year, 2024);
    assert_eq!(dt.month, 6);
    assert_eq!(dt.day, 15);
    assert_eq!(dt.hour, 14);
    assert_eq!(dt.minute, 30);
    assert_eq!(dt.second, 45);
}

#[test]
fn test_datetime_utc_from_ymd() {
    let dt = DateTimeUtc::from_ymd(2024, 12, 25);
    assert_eq!(dt.year, 2024);
    assert_eq!(dt.month, 12);
    assert_eq!(dt.day, 25);
    assert_eq!(dt.hour, 0);
    assert_eq!(dt.minute, 0);
    assert_eq!(dt.second, 0);
}

#[test]
fn test_datetime_utc_validate_valid() {
    // Valid date
    assert!(DateTimeUtc::new(2024, 6, 15, 14, 30, 45).validate().is_ok());

    // Edge cases - start of day
    assert!(DateTimeUtc::new(2024, 1, 1, 0, 0, 0).validate().is_ok());

    // Edge cases - end of day
    assert!(
        DateTimeUtc::new(2024, 12, 31, 23, 59, 59)
            .validate()
            .is_ok()
    );
}

#[test]
fn test_datetime_utc_validate_invalid_month() {
    // Month 0
    assert!(DateTimeUtc::new(2024, 0, 15, 12, 0, 0).validate().is_err());

    // Month 13
    assert!(DateTimeUtc::new(2024, 13, 15, 12, 0, 0).validate().is_err());
}

#[test]
fn test_datetime_utc_validate_invalid_day() {
    // Day 0
    assert!(DateTimeUtc::new(2024, 6, 0, 12, 0, 0).validate().is_err());

    // Day 32 in a 31-day month
    assert!(DateTimeUtc::new(2024, 1, 32, 12, 0, 0).validate().is_err());

    // Day 31 in a 30-day month
    assert!(DateTimeUtc::new(2024, 4, 31, 12, 0, 0).validate().is_err());

    // Day 30 in February (leap year)
    assert!(DateTimeUtc::new(2024, 2, 30, 12, 0, 0).validate().is_err());

    // Day 29 in February (non-leap year)
    assert!(DateTimeUtc::new(2023, 2, 29, 12, 0, 0).validate().is_err());
}

#[test]
fn test_datetime_utc_validate_leap_year() {
    // Leap year - Feb 29 is valid
    assert!(DateTimeUtc::new(2024, 2, 29, 12, 0, 0).validate().is_ok());
    assert!(DateTimeUtc::new(2000, 2, 29, 12, 0, 0).validate().is_ok()); // divisible by 400

    // Non-leap year - Feb 29 is invalid
    assert!(DateTimeUtc::new(2023, 2, 29, 12, 0, 0).validate().is_err());
    assert!(DateTimeUtc::new(1900, 2, 29, 12, 0, 0).validate().is_err()); // divisible by 100 but not 400
}

#[test]
fn test_datetime_utc_validate_invalid_hour() {
    // Hour 24
    assert!(DateTimeUtc::new(2024, 6, 15, 24, 0, 0).validate().is_err());
}

#[test]
fn test_datetime_utc_validate_invalid_minute() {
    // Minute 60
    assert!(DateTimeUtc::new(2024, 6, 15, 12, 60, 0).validate().is_err());
}

#[test]
fn test_datetime_utc_validate_invalid_second() {
    // Second 60
    assert!(
        DateTimeUtc::new(2024, 6, 15, 12, 30, 60)
            .validate()
            .is_err()
    );
}

#[test]
fn test_datetime_utc_to_rfc2822() {
    // Test a known date
    let dt = DateTimeUtc::new(2024, 1, 15, 10, 30, 45);
    let rfc2822 = dt.to_rfc2822();

    // Should contain date parts
    assert!(rfc2822.contains("15"));
    assert!(rfc2822.contains("Jan"));
    assert!(rfc2822.contains("2024"));
    assert!(rfc2822.contains("10:30:45"));
    assert!(rfc2822.contains("GMT"));
}

#[test]
fn test_datetime_utc_to_rfc2822_format() {
    let dt = DateTimeUtc::new(2024, 6, 15, 14, 30, 45);
    let rfc2822 = dt.to_rfc2822();

    // Check the general format: "Day, DD Mon YYYY HH:MM:SS GMT"
    let parts: Vec<&str> = rfc2822.split(' ').collect();
    assert_eq!(parts.len(), 6);
    assert!(parts[0].ends_with(','));
    assert_eq!(parts[5], "GMT");
}

#[test]
fn test_datetime_utc_all_months() {
    let months = [
        (1, "Jan"),
        (2, "Feb"),
        (3, "Mar"),
        (4, "Apr"),
        (5, "May"),
        (6, "Jun"),
        (7, "Jul"),
        (8, "Aug"),
        (9, "Sep"),
        (10, "Oct"),
        (11, "Nov"),
        (12, "Dec"),
    ];

    for (month_num, month_name) in months {
        let dt = DateTimeUtc::new(2024, month_num, 15, 12, 0, 0);
        assert!(dt.validate().is_ok());
        let rfc2822 = dt.to_rfc2822();
        assert!(
            rfc2822.contains(month_name),
            "Month {} should contain {}",
            month_num,
            month_name
        );
    }
}

#[test]
fn test_typst_element_text() {
    let json = r#"{ "func": "text", "text": "Hello World" }"#;
    let elem: TypstElement = serde_json::from_str(json).unwrap();
    assert!(matches!(elem, TypstElement::Text { text } if text == "Hello World"));
}

#[test]
fn test_typst_element_space() {
    let json = r#"{ "func": "space" }"#;
    let elem: TypstElement = serde_json::from_str(json).unwrap();
    assert!(matches!(elem, TypstElement::Space));
}

#[test]
fn test_typst_element_linebreak() {
    let json = r#"{ "func": "linebreak" }"#;
    let elem: TypstElement = serde_json::from_str(json).unwrap();
    assert!(matches!(elem, TypstElement::Linebreak));
}

#[test]
fn test_typst_element_strike() {
    let json = r#"{ "func": "strike", "text": "strikethrough" }"#;
    let elem: TypstElement = serde_json::from_str(json).unwrap();
    assert!(matches!(elem, TypstElement::Strike { text } if text == "strikethrough"));
}

#[test]
fn test_typst_element_link() {
    let json = r#"{ "func": "link", "dest": "https://example.com", "body": { "func": "text", "text": "link text" } }"#;
    let elem: TypstElement = serde_json::from_str(json).unwrap();

    if let TypstElement::Link { dest, body } = elem {
        assert_eq!(dest, "https://example.com");
        assert!(matches!(*body, TypstElement::Text { text } if text == "link text"));
    } else {
        panic!("Expected Link element");
    }
}

#[test]
fn test_typst_element_unknown_ignored() {
    let json = r#"{ "func": "custom_unknown_func" }"#;
    let elem: TypstElement = serde_json::from_str(json).unwrap();
    assert!(matches!(elem, TypstElement::Unknown));
}

#[test]
fn test_typst_element_sequence() {
    let json = r#"{
        "func": "sequence",
        "children": [
            { "func": "text", "text": "Hello" },
            { "func": "space" },
            { "func": "text", "text": "World" }
        ]
    }"#;
    let elem: TypstElement = serde_json::from_str(json).unwrap();

    if let TypstElement::Sequence { children } = elem {
        assert_eq!(children.len(), 3);
        assert!(matches!(&children[0], TypstElement::Text { text } if text == "Hello"));
        assert!(matches!(&children[1], TypstElement::Space));
        assert!(matches!(&children[2], TypstElement::Text { text } if text == "World"));
    } else {
        panic!("Expected Sequence element");
    }
}
