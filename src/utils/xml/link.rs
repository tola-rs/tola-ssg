use crate::config::SiteConfig;
use crate::utils::slug::{slugify_fragment, slugify_path};
use anyhow::Result;
use std::borrow::Cow;
use std::str;

use super::assets::is_asset_link;

/// Process a link value (href or src attribute).
///
/// # Link Type Detection
///
/// | Prefix | Type | Handler |
/// |--------|------|---------|
/// | `/` or `//` | Absolute | `process_absolute_link` |
/// | `#` | Fragment | `process_fragment_link` |
/// | `../` or `../../` | Relative | `process_relative_or_external_link` |
/// | `https://` | External | kept unchanged |
pub fn process_link_value(value: &[u8], config: &'static SiteConfig) -> Result<Cow<'static, [u8]>> {
    let value_str = str::from_utf8(value)?;
    let processed = match value_str.bytes().next() {
        Some(b'/') => process_absolute_link(value_str, config)?,
        Some(b'#') => process_fragment_link(value_str, config)?,
        Some(_) => process_relative_or_external_link(value_str)?,
        None => anyhow::bail!("empty link URL found in typst file"),
    };
    Ok(Cow::Owned(processed.into_bytes()))
}

/// Process absolute links (starting with `/` or `//`).
///
/// # Examples
///
/// | Input | Output (path_prefix="") |
/// |-------|----------------------|
/// | `/about` | `/about` |
/// | `/about#team` | `/about#team` (fragment slugified) |
/// | `//example.com` | `//example.com` (protocol-relative) |
pub fn process_absolute_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    let path_prefix = &config.build.path_prefix;

    if is_asset_link(value, config) {
        let value = value.trim_start_matches('/');
        return Ok(format!("/{}", path_prefix.join(value).display()));
    }

    let (path, fragment) = value.split_once('#').unwrap_or((value, ""));
    let path = path.trim_start_matches('/');
    let slugified_path = slugify_path(path, config);

    let mut result = format!("/{}", path_prefix.join(&slugified_path).display());
    if !fragment.is_empty() {
        result.push('#');
        result.push_str(&slugify_fragment(fragment, config));
    }
    Ok(result)
}

/// Process fragment links (starting with `#`).
pub fn process_fragment_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    Ok(format!("#{}", slugify_fragment(&value[1..], config)))
}

/// Process relative or external links.
///
/// # Examples
///
/// | Input | Output |
/// |-------|--------|
/// | `../images/logo.png` | `../../images/logo.png` |
/// | `../../assets/doc.pdf` | `../../../assets/doc.pdf` |
/// | `other.html` | `../other.html` |
/// | `https://example.com` | `https://example.com` (unchanged) |
///
/// Note: Relative links get `../` prepended because content pages
/// are at `/post/index.html`, so need to go up one level first.
pub fn process_relative_or_external_link(value: &str) -> Result<String> {
    Ok(if is_external_link(value) {
        value.to_string()
    } else {
        format!("../{value}")
    })
}

/// Check if a link is external (has a scheme like http:, mailto:, etc.)
#[inline]
pub fn is_external_link(link: &str) -> bool {
    link.find(':').is_some_and(|pos| {
        link[..pos]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}
