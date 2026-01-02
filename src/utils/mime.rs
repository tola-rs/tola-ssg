//! MIME type detection utilities.
//!
//! Provides consistent MIME type detection across the codebase.

#![allow(dead_code)]

use std::path::Path;

/// Common MIME type constants.
pub mod types {
    // Text
    pub const HTML: &str = "text/html; charset=utf-8";
    pub const PLAIN: &str = "text/plain; charset=utf-8";
    pub const CSS: &str = "text/css; charset=utf-8";
    pub const JAVASCRIPT: &str = "text/javascript; charset=utf-8";
    pub const TYPESCRIPT: &str = "text/typescript; charset=utf-8";
    pub const JSON: &str = "application/json";
    pub const XML: &str = "application/xml";
    pub const MARKDOWN: &str = "text/markdown; charset=utf-8";
    pub const YAML: &str = "text/yaml; charset=utf-8";
    pub const TOML: &str = "text/toml; charset=utf-8";
    pub const CSV: &str = "text/csv; charset=utf-8";

    // Web feeds
    pub const RSS: &str = "application/rss+xml";
    pub const ATOM: &str = "application/atom+xml";

    // Documents
    pub const PDF: &str = "application/pdf";

    // Binary
    pub const OCTET_STREAM: &str = "application/octet-stream";
    pub const WASM: &str = "application/wasm";
    pub const ZIP: &str = "application/zip";
    pub const GZIP: &str = "application/gzip";

    // Images
    pub const PNG: &str = "image/png";
    pub const JPEG: &str = "image/jpeg";
    pub const GIF: &str = "image/gif";
    pub const WEBP: &str = "image/webp";
    pub const AVIF: &str = "image/avif";
    pub const SVG: &str = "image/svg+xml";
    pub const ICO: &str = "image/x-icon";
    pub const BMP: &str = "image/bmp";
    pub const TIFF: &str = "image/tiff";

    // Audio
    pub const MP3: &str = "audio/mpeg";
    pub const WAV: &str = "audio/wav";
    pub const OGG_AUDIO: &str = "audio/ogg";
    pub const FLAC: &str = "audio/flac";
    pub const AAC: &str = "audio/aac";
    pub const WEBM_AUDIO: &str = "audio/webm";

    // Video
    pub const MP4: &str = "video/mp4";
    pub const WEBM: &str = "video/webm";
    pub const OGG_VIDEO: &str = "video/ogg";
    pub const AVI: &str = "video/x-msvideo";
    pub const MOV: &str = "video/quicktime";

    // Fonts
    pub const WOFF: &str = "font/woff";
    pub const WOFF2: &str = "font/woff2";
    pub const TTF: &str = "font/ttf";
    pub const OTF: &str = "font/otf";
    pub const EOT: &str = "application/vnd.ms-fontobject";
}

/// Guess MIME type from file extension.
///
/// Returns a full MIME type string suitable for HTTP Content-Type header.
pub fn from_path(path: &Path) -> &'static str {
    from_extension(path.extension().and_then(|e| e.to_str()))
}

/// Guess MIME type from file extension string.
pub fn from_extension(ext: Option<&str>) -> &'static str {
    match ext {
        // Web / Text
        Some("html" | "htm") => types::HTML,
        Some("css") => types::CSS,
        Some("js" | "mjs" | "cjs") => types::JAVASCRIPT,
        Some("ts" | "tsx" | "mts" | "cts") => types::TYPESCRIPT,
        Some("json") => types::JSON,
        Some("xml") => types::XML,
        Some("yaml" | "yml") => types::YAML,
        Some("toml") => types::TOML,
        Some("csv") => types::CSV,

        // Web feeds
        Some("rss") => types::RSS,
        Some("atom") => types::ATOM,

        // Images
        Some("svg") => types::SVG,
        Some("png") => types::PNG,
        Some("jpg" | "jpeg") => types::JPEG,
        Some("gif") => types::GIF,
        Some("webp") => types::WEBP,
        Some("avif") => types::AVIF,
        Some("ico") => types::ICO,
        Some("bmp") => types::BMP,
        Some("tif" | "tiff") => types::TIFF,

        // Audio
        Some("mp3") => types::MP3,
        Some("wav") => types::WAV,
        Some("ogg" | "oga") => types::OGG_AUDIO,
        Some("flac") => types::FLAC,
        Some("aac" | "m4a") => types::AAC,

        // Video
        Some("mp4" | "m4v") => types::MP4,
        Some("webm") => types::WEBM,
        Some("ogv") => types::OGG_VIDEO,
        Some("avi") => types::AVI,
        Some("mov") => types::MOV,

        // Fonts
        Some("woff") => types::WOFF,
        Some("woff2") => types::WOFF2,
        Some("ttf") => types::TTF,
        Some("otf") => types::OTF,
        Some("eot") => types::EOT,

        // Documents / Binary
        Some("pdf") => types::PDF,
        Some("txt") => types::PLAIN,
        Some("md") => types::MARKDOWN,
        Some("wasm") => types::WASM,
        Some("zip") => types::ZIP,
        Some("gz" | "gzip") => types::GZIP,

        _ => types::OCTET_STREAM,
    }
}

/// Get MIME type for favicon/icon files.
///
/// This is a specialized version that defaults to `image/x-icon` for unknown types,
/// which is appropriate for favicon files.
pub fn for_icon(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()) {
        Some(ext) => match ext.as_str() {
            "png" => types::PNG,
            "svg" => types::SVG,
            "avif" => types::AVIF,
            "webp" => types::WEBP,
            "gif" => types::GIF,
            "jpg" | "jpeg" => types::JPEG,
            _ => types::ICO,
        },
        None => types::ICO,
    }
}

/// Check if the MIME type represents text content.
pub fn is_text(mime: &str) -> bool {
    mime.starts_with("text/") || mime == types::JSON || mime == types::XML
}

/// Check if the MIME type represents an image.
pub fn is_image(mime: &str) -> bool {
    mime.starts_with("image/")
}

/// Check if the MIME type represents audio.
pub fn is_audio(mime: &str) -> bool {
    mime.starts_with("audio/")
}

/// Check if the MIME type represents video.
pub fn is_video(mime: &str) -> bool {
    mime.starts_with("video/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_from_path() {
        assert_eq!(from_path(&PathBuf::from("index.html")), types::HTML);
        assert_eq!(from_path(&PathBuf::from("style.css")), types::CSS);
        assert_eq!(from_path(&PathBuf::from("app.js")), types::JAVASCRIPT);
        assert_eq!(from_path(&PathBuf::from("app.ts")), types::TYPESCRIPT);
        assert_eq!(from_path(&PathBuf::from("logo.png")), types::PNG);
        assert_eq!(from_path(&PathBuf::from("photo.jpeg")), types::JPEG);
        assert_eq!(from_path(&PathBuf::from("icon.svg")), types::SVG);
        assert_eq!(from_path(&PathBuf::from("video.mp4")), types::MP4);
        assert_eq!(from_path(&PathBuf::from("audio.mp3")), types::MP3);
        assert_eq!(from_path(&PathBuf::from("unknown.xyz")), types::OCTET_STREAM);
    }

    #[test]
    fn test_for_icon() {
        assert_eq!(for_icon(&PathBuf::from("favicon.ico")), types::ICO);
        assert_eq!(for_icon(&PathBuf::from("favicon.png")), types::PNG);
        assert_eq!(for_icon(&PathBuf::from("favicon.svg")), types::SVG);
        assert_eq!(for_icon(&PathBuf::from("favicon.unknown")), types::ICO);
    }

    #[test]
    fn test_is_text() {
        assert!(is_text(types::HTML));
        assert!(is_text(types::CSS));
        assert!(is_text(types::JSON));
        assert!(is_text(types::XML));
        assert!(!is_text(types::PNG));
        assert!(!is_text(types::MP4));
    }

    #[test]
    fn test_is_media() {
        assert!(is_image(types::PNG));
        assert!(is_image(types::SVG));
        assert!(is_audio(types::MP3));
        assert!(is_video(types::MP4));
        assert!(!is_image(types::HTML));
    }
}
