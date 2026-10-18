//! Producer-declared output meaning and deployment-neutral response metadata.

use std::borrow::Cow;
use std::path::Path;

use serde::Serialize;

use super::graph::OutputKind;
use tola_address::OutputPath;

/// Producer-declared content semantics, independent of how a reference uses the output.
/// An opaque Bundle asset remains opaque even when its extension determines a media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclaredOutputSemantics {
    HtmlDocument,
    PdfDocument,
    PngDocument,
    SvgDocument,
    Html,
    Css,
    JavaScript,
    JavaScriptModule,
    TypeScript,
    Json,
    Xml,
    Yaml,
    Toml,
    Csv,
    PlainText,
    Image,
    Audio,
    Video,
    Font,
    WebAssembly,
    Archive,
    RssFeed,
    AtomFeed,
    JsonFeed,
    Sitemap,
    Opaque,
}

impl DeclaredOutputSemantics {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HtmlDocument => "html-document",
            Self::PdfDocument => "pdf-document",
            Self::PngDocument => "png-document",
            Self::SvgDocument => "svg-document",
            Self::Html => "html",
            Self::Css => "css",
            Self::JavaScript => "javascript",
            Self::JavaScriptModule => "javascript-module",
            Self::TypeScript => "typescript",
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Csv => "csv",
            Self::PlainText => "plain-text",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Font => "font",
            Self::WebAssembly => "web-assembly",
            Self::Archive => "archive",
            Self::RssFeed => "rss-feed",
            Self::AtomFeed => "atom-feed",
            Self::JsonFeed => "json-feed",
            Self::Sitemap => "sitemap",
            Self::Opaque => "opaque",
        }
    }
}

/// A validated concrete media type for an output response.
///
/// Parsing accepts any MIME type and parameters, independently of the output
/// kind. Common constants avoid allocation; parsed values retain their header.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ResponseMediaType(Cow<'static, str>);

/// A response media type is malformed or contains a wildcard.
#[derive(Debug, thiserror::Error)]
pub enum InvalidMediaType {
    // The library's own parse text is foreign to this site's diagnostics, so only the
    // category is rendered; the value passed to `ResponseMediaType::parse` is not retained.
    #[error("response media type is not a valid MIME type")]
    Syntax(#[from] mime::FromStrError),
    #[error("response media type must have a concrete type and subtype")]
    NotConcrete,
}

impl ResponseMediaType {
    pub const HTML: Self = Self(Cow::Borrowed("text/html; charset=utf-8"));
    pub const PLAIN_TEXT: Self = Self(Cow::Borrowed("text/plain; charset=utf-8"));
    pub const CSS: Self = Self(Cow::Borrowed("text/css; charset=utf-8"));
    pub const JAVASCRIPT: Self = Self(Cow::Borrowed("text/javascript; charset=utf-8"));
    pub const TYPESCRIPT: Self = Self(Cow::Borrowed("text/typescript; charset=utf-8"));
    pub const JSON: Self = Self(Cow::Borrowed("application/json"));
    pub const JSON_FEED: Self = Self(Cow::Borrowed("application/feed+json"));
    pub const XML: Self = Self(Cow::Borrowed("application/xml"));
    pub const YAML: Self = Self(Cow::Borrowed("text/yaml; charset=utf-8"));
    pub const TOML: Self = Self(Cow::Borrowed("text/toml; charset=utf-8"));
    pub const CSV: Self = Self(Cow::Borrowed("text/csv; charset=utf-8"));
    pub const RSS: Self = Self(Cow::Borrowed("application/rss+xml"));
    pub const ATOM: Self = Self(Cow::Borrowed("application/atom+xml"));
    pub const PDF: Self = Self(Cow::Borrowed("application/pdf"));
    pub const OCTET_STREAM: Self = Self(Cow::Borrowed("application/octet-stream"));
    pub const WEB_ASSEMBLY: Self = Self(Cow::Borrowed("application/wasm"));
    pub const ZIP: Self = Self(Cow::Borrowed("application/zip"));
    pub const GZIP: Self = Self(Cow::Borrowed("application/gzip"));
    pub const PNG: Self = Self(Cow::Borrowed("image/png"));
    pub const JPEG: Self = Self(Cow::Borrowed("image/jpeg"));
    pub const GIF: Self = Self(Cow::Borrowed("image/gif"));
    pub const WEBP: Self = Self(Cow::Borrowed("image/webp"));
    pub const AVIF: Self = Self(Cow::Borrowed("image/avif"));
    pub const SVG: Self = Self(Cow::Borrowed("image/svg+xml"));
    pub const ICON: Self = Self(Cow::Borrowed("image/x-icon"));
    pub const BMP: Self = Self(Cow::Borrowed("image/bmp"));
    pub const TIFF: Self = Self(Cow::Borrowed("image/tiff"));
    pub const MP3: Self = Self(Cow::Borrowed("audio/mpeg"));
    pub const WAV: Self = Self(Cow::Borrowed("audio/wav"));
    pub const OGG_AUDIO: Self = Self(Cow::Borrowed("audio/ogg"));
    pub const FLAC: Self = Self(Cow::Borrowed("audio/flac"));
    pub const AAC: Self = Self(Cow::Borrowed("audio/aac"));
    pub const MP4: Self = Self(Cow::Borrowed("video/mp4"));
    pub const WEBM: Self = Self(Cow::Borrowed("video/webm"));
    pub const OGG_VIDEO: Self = Self(Cow::Borrowed("video/ogg"));
    pub const AVI: Self = Self(Cow::Borrowed("video/x-msvideo"));
    pub const QUICK_TIME: Self = Self(Cow::Borrowed("video/quicktime"));
    pub const WOFF: Self = Self(Cow::Borrowed("font/woff"));
    pub const WOFF2: Self = Self(Cow::Borrowed("font/woff2"));
    pub const TTF: Self = Self(Cow::Borrowed("font/ttf"));
    pub const OTF: Self = Self(Cow::Borrowed("font/otf"));
    pub const EOT: Self = Self(Cow::Borrowed("application/vnd.ms-fontobject"));

    /// Parse a concrete response MIME type, including its parameters.
    pub fn parse(input: &str) -> Result<Self, InvalidMediaType> {
        let media_type: mime::Mime = input.parse()?;
        if [media_type.type_(), media_type.subtype()]
            .iter()
            .any(|name| name.as_str().is_empty() || name.as_str().contains('*'))
        {
            return Err(InvalidMediaType::NotConcrete);
        }
        Ok(Self(Cow::Owned(media_type.to_string())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Type and subtype, without parameters such as the character set.
    pub fn essence(&self) -> &str {
        self.0
            .split_once(';')
            .map_or(self.0.as_ref(), |(essence, _)| essence)
            .trim_end()
    }
}

impl std::str::FromStr for ResponseMediaType {
    type Err = InvalidMediaType;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl std::fmt::Display for ResponseMediaType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The complete producer declaration of one candidate output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputDeclaration {
    semantics: DeclaredOutputSemantics,
    media_type: ResponseMediaType,
}

impl OutputDeclaration {
    const fn new(semantics: DeclaredOutputSemantics, media_type: ResponseMediaType) -> Self {
        Self {
            semantics,
            media_type,
        }
    }

    pub(crate) const fn html_document() -> Self {
        Self::new(
            DeclaredOutputSemantics::HtmlDocument,
            ResponseMediaType::HTML,
        )
    }

    pub(crate) const fn pdf_document() -> Self {
        Self::new(DeclaredOutputSemantics::PdfDocument, ResponseMediaType::PDF)
    }

    pub(crate) const fn png_document() -> Self {
        Self::new(DeclaredOutputSemantics::PngDocument, ResponseMediaType::PNG)
    }

    pub(crate) const fn svg_document() -> Self {
        Self::new(DeclaredOutputSemantics::SvgDocument, ResponseMediaType::SVG)
    }

    pub(crate) const fn image(media_type: ResponseMediaType) -> Self {
        Self::new(DeclaredOutputSemantics::Image, media_type)
    }

    pub(crate) const fn rss_feed() -> Self {
        Self::new(DeclaredOutputSemantics::RssFeed, ResponseMediaType::RSS)
    }

    pub(crate) const fn atom_feed() -> Self {
        Self::new(DeclaredOutputSemantics::AtomFeed, ResponseMediaType::ATOM)
    }

    pub(crate) const fn json_feed() -> Self {
        Self::new(
            DeclaredOutputSemantics::JsonFeed,
            ResponseMediaType::JSON_FEED,
        )
    }

    pub(crate) const fn sitemap() -> Self {
        Self::new(DeclaredOutputSemantics::Sitemap, ResponseMediaType::XML)
    }

    pub(crate) const fn opaque(media_type: ResponseMediaType) -> Self {
        Self::new(DeclaredOutputSemantics::Opaque, media_type)
    }

    /// Infer asset semantics from the logical source extension, not the mapped output path.
    pub(crate) fn from_filesystem_source(path: &Path) -> Self {
        let extension = path.extension().and_then(|extension| extension.to_str());
        filesystem_declaration_for_extension(extension)
    }

    /// Select an opaque file response by logical output extension.
    pub(crate) fn opaque_from_output_path(path: &OutputPath) -> Self {
        let extension = path
            .as_str()
            .rsplit_once('.')
            .map(|(_, extension)| extension)
            .filter(|extension| !extension.contains('/'));
        Self::opaque(media_type_for_extension(extension))
    }

    pub const fn semantics(&self) -> DeclaredOutputSemantics {
        self.semantics
    }

    /// Derive the graph's document-or-asset view from the producer declaration.
    pub const fn kind(&self) -> OutputKind {
        match self.semantics {
            DeclaredOutputSemantics::HtmlDocument => OutputKind::HtmlDocument,
            DeclaredOutputSemantics::PdfDocument => OutputKind::PdfDocument,
            DeclaredOutputSemantics::PngDocument => OutputKind::PngDocument,
            DeclaredOutputSemantics::SvgDocument => OutputKind::SvgDocument,
            DeclaredOutputSemantics::Html
            | DeclaredOutputSemantics::Css
            | DeclaredOutputSemantics::JavaScript
            | DeclaredOutputSemantics::JavaScriptModule
            | DeclaredOutputSemantics::TypeScript
            | DeclaredOutputSemantics::Json
            | DeclaredOutputSemantics::Xml
            | DeclaredOutputSemantics::Yaml
            | DeclaredOutputSemantics::Toml
            | DeclaredOutputSemantics::Csv
            | DeclaredOutputSemantics::PlainText
            | DeclaredOutputSemantics::Image
            | DeclaredOutputSemantics::Audio
            | DeclaredOutputSemantics::Video
            | DeclaredOutputSemantics::Font
            | DeclaredOutputSemantics::WebAssembly
            | DeclaredOutputSemantics::Archive
            | DeclaredOutputSemantics::RssFeed
            | DeclaredOutputSemantics::AtomFeed
            | DeclaredOutputSemantics::JsonFeed
            | DeclaredOutputSemantics::Sitemap
            | DeclaredOutputSemantics::Opaque => OutputKind::Asset,
        }
    }

    pub const fn media_type(&self) -> &ResponseMediaType {
        &self.media_type
    }
}

/// The semantics and media type one extension names; unknown extensions stay opaque.
fn extension_semantics(extension: &str) -> (DeclaredOutputSemantics, ResponseMediaType) {
    match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => (DeclaredOutputSemantics::Html, ResponseMediaType::HTML),
        "css" => (DeclaredOutputSemantics::Css, ResponseMediaType::CSS),
        "mjs" => (
            DeclaredOutputSemantics::JavaScriptModule,
            ResponseMediaType::JAVASCRIPT,
        ),
        "js" | "cjs" => (
            DeclaredOutputSemantics::JavaScript,
            ResponseMediaType::JAVASCRIPT,
        ),
        "ts" | "tsx" | "mts" | "cts" => (
            DeclaredOutputSemantics::TypeScript,
            ResponseMediaType::TYPESCRIPT,
        ),
        "json" | "map" | "webmanifest" => (DeclaredOutputSemantics::Json, ResponseMediaType::JSON),
        "xml" => (DeclaredOutputSemantics::Xml, ResponseMediaType::XML),
        "yaml" | "yml" => (DeclaredOutputSemantics::Yaml, ResponseMediaType::YAML),
        "toml" => (DeclaredOutputSemantics::Toml, ResponseMediaType::TOML),
        "csv" => (DeclaredOutputSemantics::Csv, ResponseMediaType::CSV),
        "txt" => (
            DeclaredOutputSemantics::PlainText,
            ResponseMediaType::PLAIN_TEXT,
        ),
        "svg" => (DeclaredOutputSemantics::Image, ResponseMediaType::SVG),
        "png" => (DeclaredOutputSemantics::Image, ResponseMediaType::PNG),
        "jpg" | "jpeg" => (DeclaredOutputSemantics::Image, ResponseMediaType::JPEG),
        "gif" => (DeclaredOutputSemantics::Image, ResponseMediaType::GIF),
        "webp" => (DeclaredOutputSemantics::Image, ResponseMediaType::WEBP),
        "avif" => (DeclaredOutputSemantics::Image, ResponseMediaType::AVIF),
        "ico" => (DeclaredOutputSemantics::Image, ResponseMediaType::ICON),
        "bmp" => (DeclaredOutputSemantics::Image, ResponseMediaType::BMP),
        "tif" | "tiff" => (DeclaredOutputSemantics::Image, ResponseMediaType::TIFF),
        "mp3" => (DeclaredOutputSemantics::Audio, ResponseMediaType::MP3),
        "wav" => (DeclaredOutputSemantics::Audio, ResponseMediaType::WAV),
        "ogg" | "oga" => (DeclaredOutputSemantics::Audio, ResponseMediaType::OGG_AUDIO),
        "flac" => (DeclaredOutputSemantics::Audio, ResponseMediaType::FLAC),
        "aac" | "m4a" => (DeclaredOutputSemantics::Audio, ResponseMediaType::AAC),
        "mp4" | "m4v" => (DeclaredOutputSemantics::Video, ResponseMediaType::MP4),
        "webm" => (DeclaredOutputSemantics::Video, ResponseMediaType::WEBM),
        "ogv" => (DeclaredOutputSemantics::Video, ResponseMediaType::OGG_VIDEO),
        "avi" => (DeclaredOutputSemantics::Video, ResponseMediaType::AVI),
        "mov" => (
            DeclaredOutputSemantics::Video,
            ResponseMediaType::QUICK_TIME,
        ),
        "woff" => (DeclaredOutputSemantics::Font, ResponseMediaType::WOFF),
        "woff2" => (DeclaredOutputSemantics::Font, ResponseMediaType::WOFF2),
        "ttf" => (DeclaredOutputSemantics::Font, ResponseMediaType::TTF),
        "otf" => (DeclaredOutputSemantics::Font, ResponseMediaType::OTF),
        "eot" => (DeclaredOutputSemantics::Font, ResponseMediaType::EOT),
        "wasm" => (
            DeclaredOutputSemantics::WebAssembly,
            ResponseMediaType::WEB_ASSEMBLY,
        ),
        "zip" => (DeclaredOutputSemantics::Archive, ResponseMediaType::ZIP),
        "gz" | "gzip" => (DeclaredOutputSemantics::Archive, ResponseMediaType::GZIP),
        "rss" => (DeclaredOutputSemantics::Opaque, ResponseMediaType::RSS),
        "atom" => (DeclaredOutputSemantics::Opaque, ResponseMediaType::ATOM),
        "pdf" => (DeclaredOutputSemantics::Opaque, ResponseMediaType::PDF),
        _ => (
            DeclaredOutputSemantics::Opaque,
            ResponseMediaType::OCTET_STREAM,
        ),
    }
}

fn filesystem_declaration_for_extension(extension: Option<&str>) -> OutputDeclaration {
    let Some(extension) = extension else {
        return OutputDeclaration::opaque(ResponseMediaType::OCTET_STREAM);
    };
    let (semantics, media_type) = extension_semantics(extension);
    OutputDeclaration::new(semantics, media_type)
}

fn media_type_for_extension(extension: Option<&str>) -> ResponseMediaType {
    match extension {
        Some(extension) => extension_semantics(extension).1,
        None => ResponseMediaType::OCTET_STREAM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_media_types_keep_parameters() {
        for raw in [
            "text/vtt",
            "application/manifest+json",
            "application/vnd.example+json; version=2",
        ] {
            let media_type = ResponseMediaType::parse(raw).unwrap();
            assert_eq!(media_type.as_str(), raw);
            assert_eq!(serde_json::to_value(&media_type).unwrap(), raw);
        }
        let css = ResponseMediaType::parse("TEXT/CSS; charset=iso-8859-1").unwrap();
        assert_eq!(css.essence(), ResponseMediaType::CSS.essence());
        assert_ne!(css, ResponseMediaType::CSS);
        for raw in [
            "*/*",
            "text/*",
            "text/",
            "text/css\r\nx-injected: yes",
            "text/css, application/json",
        ] {
            assert!(ResponseMediaType::parse(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn declaration_decides_the_output_kind() {
        for (declaration, kind) in [
            (OutputDeclaration::html_document(), OutputKind::HtmlDocument),
            (OutputDeclaration::pdf_document(), OutputKind::PdfDocument),
            (OutputDeclaration::png_document(), OutputKind::PngDocument),
            (OutputDeclaration::svg_document(), OutputKind::SvgDocument),
            (
                OutputDeclaration::new(
                    DeclaredOutputSemantics::Css,
                    ResponseMediaType::OCTET_STREAM,
                ),
                OutputKind::Asset,
            ),
        ] {
            assert_eq!(declaration.kind(), kind);
        }
    }

    #[test]
    fn extension_names_the_declared_semantics() {
        let declaration = OutputDeclaration::from_filesystem_source(Path::new("source/app.css"));
        assert_eq!(declaration.semantics(), DeclaredOutputSemantics::Css);
        assert_eq!(declaration.media_type(), &ResponseMediaType::CSS);

        let stylesheet = OutputPath::parse("styles/app.css").unwrap();
        assert_eq!(
            OutputDeclaration::opaque_from_output_path(&stylesheet).semantics(),
            DeclaredOutputSemantics::Opaque
        );
        assert_eq!(
            OutputDeclaration::opaque_from_output_path(&stylesheet).media_type(),
            &ResponseMediaType::CSS
        );

        for path in ["download", "download.unknown"] {
            let output = OutputPath::parse(path).unwrap();
            assert_eq!(
                OutputDeclaration::opaque_from_output_path(&output).media_type(),
                &ResponseMediaType::OCTET_STREAM,
                "{path}"
            );
        }
    }
}
