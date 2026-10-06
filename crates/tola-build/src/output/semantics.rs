//! Producer-declared output meaning and deployment-neutral response metadata.

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
pub struct ResponseMediaType(std::borrow::Cow<'static, str>);

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
    pub const HTML: Self = Self(std::borrow::Cow::Borrowed("text/html; charset=utf-8"));
    pub const PLAIN_TEXT: Self = Self(std::borrow::Cow::Borrowed("text/plain; charset=utf-8"));
    pub const CSS: Self = Self(std::borrow::Cow::Borrowed("text/css; charset=utf-8"));
    pub const JAVASCRIPT: Self = Self(std::borrow::Cow::Borrowed("text/javascript; charset=utf-8"));
    pub const TYPESCRIPT: Self = Self(std::borrow::Cow::Borrowed("text/typescript; charset=utf-8"));
    pub const JSON: Self = Self(std::borrow::Cow::Borrowed("application/json"));
    pub const JSON_FEED: Self = Self(std::borrow::Cow::Borrowed("application/feed+json"));
    pub const XML: Self = Self(std::borrow::Cow::Borrowed("application/xml"));
    pub const YAML: Self = Self(std::borrow::Cow::Borrowed("text/yaml; charset=utf-8"));
    pub const TOML: Self = Self(std::borrow::Cow::Borrowed("text/toml; charset=utf-8"));
    pub const CSV: Self = Self(std::borrow::Cow::Borrowed("text/csv; charset=utf-8"));
    pub const RSS: Self = Self(std::borrow::Cow::Borrowed("application/rss+xml"));
    pub const ATOM: Self = Self(std::borrow::Cow::Borrowed("application/atom+xml"));
    pub const PDF: Self = Self(std::borrow::Cow::Borrowed("application/pdf"));
    pub const OCTET_STREAM: Self = Self(std::borrow::Cow::Borrowed("application/octet-stream"));
    pub const WEB_ASSEMBLY: Self = Self(std::borrow::Cow::Borrowed("application/wasm"));
    pub const ZIP: Self = Self(std::borrow::Cow::Borrowed("application/zip"));
    pub const GZIP: Self = Self(std::borrow::Cow::Borrowed("application/gzip"));
    pub const PNG: Self = Self(std::borrow::Cow::Borrowed("image/png"));
    pub const JPEG: Self = Self(std::borrow::Cow::Borrowed("image/jpeg"));
    pub const GIF: Self = Self(std::borrow::Cow::Borrowed("image/gif"));
    pub const WEBP: Self = Self(std::borrow::Cow::Borrowed("image/webp"));
    pub const AVIF: Self = Self(std::borrow::Cow::Borrowed("image/avif"));
    pub const SVG: Self = Self(std::borrow::Cow::Borrowed("image/svg+xml"));
    pub const ICON: Self = Self(std::borrow::Cow::Borrowed("image/x-icon"));
    pub const BMP: Self = Self(std::borrow::Cow::Borrowed("image/bmp"));
    pub const TIFF: Self = Self(std::borrow::Cow::Borrowed("image/tiff"));
    pub const MP3: Self = Self(std::borrow::Cow::Borrowed("audio/mpeg"));
    pub const WAV: Self = Self(std::borrow::Cow::Borrowed("audio/wav"));
    pub const OGG_AUDIO: Self = Self(std::borrow::Cow::Borrowed("audio/ogg"));
    pub const FLAC: Self = Self(std::borrow::Cow::Borrowed("audio/flac"));
    pub const AAC: Self = Self(std::borrow::Cow::Borrowed("audio/aac"));
    pub const MP4: Self = Self(std::borrow::Cow::Borrowed("video/mp4"));
    pub const WEBM: Self = Self(std::borrow::Cow::Borrowed("video/webm"));
    pub const OGG_VIDEO: Self = Self(std::borrow::Cow::Borrowed("video/ogg"));
    pub const AVI: Self = Self(std::borrow::Cow::Borrowed("video/x-msvideo"));
    pub const QUICK_TIME: Self = Self(std::borrow::Cow::Borrowed("video/quicktime"));
    pub const WOFF: Self = Self(std::borrow::Cow::Borrowed("font/woff"));
    pub const WOFF2: Self = Self(std::borrow::Cow::Borrowed("font/woff2"));
    pub const TTF: Self = Self(std::borrow::Cow::Borrowed("font/ttf"));
    pub const OTF: Self = Self(std::borrow::Cow::Borrowed("font/otf"));
    pub const EOT: Self = Self(std::borrow::Cow::Borrowed("application/vnd.ms-fontobject"));

    /// Parse a concrete response MIME type, including its parameters.
    pub fn parse(input: &str) -> Result<Self, InvalidMediaType> {
        let media_type: mime::Mime = input.parse()?;
        if [media_type.type_(), media_type.subtype()]
            .iter()
            .any(|name| name.as_str().is_empty() || name.as_str().contains('*'))
        {
            return Err(InvalidMediaType::NotConcrete);
        }
        Ok(Self(std::borrow::Cow::Owned(media_type.to_string())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Type and subtype, without parameters such as the character set.
    pub fn essence(&self) -> &str {
        self.0
            .split(';')
            .next()
            .expect("validated media type")
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

fn filesystem_declaration_for_extension(extension: Option<&str>) -> OutputDeclaration {
    let media_type = media_type_for_extension(extension);
    let semantics = if extension.is_some_and(|extension| {
        extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
    }) {
        DeclaredOutputSemantics::Html
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("css")) {
        DeclaredOutputSemantics::Css
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("mjs")) {
        DeclaredOutputSemantics::JavaScriptModule
    } else if extension.is_some_and(|extension| {
        extension.eq_ignore_ascii_case("js") || extension.eq_ignore_ascii_case("cjs")
    }) {
        DeclaredOutputSemantics::JavaScript
    } else if extension
        .is_some_and(|extension| matches_ignore_ascii_case(extension, &["ts", "tsx", "mts", "cts"]))
    {
        DeclaredOutputSemantics::TypeScript
    } else if extension.is_some_and(|extension| {
        matches_ignore_ascii_case(extension, &["json", "map", "webmanifest"])
    }) {
        DeclaredOutputSemantics::Json
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("xml")) {
        DeclaredOutputSemantics::Xml
    } else if extension
        .is_some_and(|extension| matches_ignore_ascii_case(extension, &["yaml", "yml"]))
    {
        DeclaredOutputSemantics::Yaml
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("toml")) {
        DeclaredOutputSemantics::Toml
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("csv")) {
        DeclaredOutputSemantics::Csv
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("txt")) {
        DeclaredOutputSemantics::PlainText
    } else if extension.is_some_and(|extension| {
        matches_ignore_ascii_case(
            extension,
            &[
                "svg", "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "bmp", "tif", "tiff",
            ],
        )
    }) {
        DeclaredOutputSemantics::Image
    } else if extension.is_some_and(|extension| {
        matches_ignore_ascii_case(
            extension,
            &["mp3", "wav", "ogg", "oga", "flac", "aac", "m4a"],
        )
    }) {
        DeclaredOutputSemantics::Audio
    } else if extension.is_some_and(|extension| {
        matches_ignore_ascii_case(extension, &["mp4", "m4v", "webm", "ogv", "avi", "mov"])
    }) {
        DeclaredOutputSemantics::Video
    } else if extension.is_some_and(|extension| {
        matches_ignore_ascii_case(extension, &["woff", "woff2", "ttf", "otf", "eot"])
    }) {
        DeclaredOutputSemantics::Font
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("wasm")) {
        DeclaredOutputSemantics::WebAssembly
    } else if extension
        .is_some_and(|extension| matches_ignore_ascii_case(extension, &["zip", "gz", "gzip"]))
    {
        DeclaredOutputSemantics::Archive
    } else {
        DeclaredOutputSemantics::Opaque
    };
    OutputDeclaration::new(semantics, media_type)
}

fn media_type_for_extension(extension: Option<&str>) -> ResponseMediaType {
    let Some(extension) = extension else {
        return ResponseMediaType::OCTET_STREAM;
    };
    if matches_ignore_ascii_case(extension, &["html", "htm"]) {
        ResponseMediaType::HTML
    } else if extension.eq_ignore_ascii_case("css") {
        ResponseMediaType::CSS
    } else if matches_ignore_ascii_case(extension, &["js", "mjs", "cjs"]) {
        ResponseMediaType::JAVASCRIPT
    } else if matches_ignore_ascii_case(extension, &["ts", "tsx", "mts", "cts"]) {
        ResponseMediaType::TYPESCRIPT
    } else if matches_ignore_ascii_case(extension, &["json", "map", "webmanifest"]) {
        ResponseMediaType::JSON
    } else if extension.eq_ignore_ascii_case("xml") {
        ResponseMediaType::XML
    } else if matches_ignore_ascii_case(extension, &["yaml", "yml"]) {
        ResponseMediaType::YAML
    } else if extension.eq_ignore_ascii_case("toml") {
        ResponseMediaType::TOML
    } else if extension.eq_ignore_ascii_case("csv") {
        ResponseMediaType::CSV
    } else if extension.eq_ignore_ascii_case("rss") {
        ResponseMediaType::RSS
    } else if extension.eq_ignore_ascii_case("atom") {
        ResponseMediaType::ATOM
    } else if extension.eq_ignore_ascii_case("pdf") {
        ResponseMediaType::PDF
    } else if extension.eq_ignore_ascii_case("wasm") {
        ResponseMediaType::WEB_ASSEMBLY
    } else if extension.eq_ignore_ascii_case("zip") {
        ResponseMediaType::ZIP
    } else if matches_ignore_ascii_case(extension, &["gz", "gzip"]) {
        ResponseMediaType::GZIP
    } else if extension.eq_ignore_ascii_case("png") {
        ResponseMediaType::PNG
    } else if matches_ignore_ascii_case(extension, &["jpg", "jpeg"]) {
        ResponseMediaType::JPEG
    } else if extension.eq_ignore_ascii_case("gif") {
        ResponseMediaType::GIF
    } else if extension.eq_ignore_ascii_case("webp") {
        ResponseMediaType::WEBP
    } else if extension.eq_ignore_ascii_case("avif") {
        ResponseMediaType::AVIF
    } else if extension.eq_ignore_ascii_case("svg") {
        ResponseMediaType::SVG
    } else if extension.eq_ignore_ascii_case("ico") {
        ResponseMediaType::ICON
    } else if extension.eq_ignore_ascii_case("bmp") {
        ResponseMediaType::BMP
    } else if matches_ignore_ascii_case(extension, &["tif", "tiff"]) {
        ResponseMediaType::TIFF
    } else if extension.eq_ignore_ascii_case("mp3") {
        ResponseMediaType::MP3
    } else if extension.eq_ignore_ascii_case("wav") {
        ResponseMediaType::WAV
    } else if matches_ignore_ascii_case(extension, &["ogg", "oga"]) {
        ResponseMediaType::OGG_AUDIO
    } else if extension.eq_ignore_ascii_case("flac") {
        ResponseMediaType::FLAC
    } else if matches_ignore_ascii_case(extension, &["aac", "m4a"]) {
        ResponseMediaType::AAC
    } else if matches_ignore_ascii_case(extension, &["mp4", "m4v"]) {
        ResponseMediaType::MP4
    } else if extension.eq_ignore_ascii_case("webm") {
        ResponseMediaType::WEBM
    } else if extension.eq_ignore_ascii_case("ogv") {
        ResponseMediaType::OGG_VIDEO
    } else if extension.eq_ignore_ascii_case("avi") {
        ResponseMediaType::AVI
    } else if extension.eq_ignore_ascii_case("mov") {
        ResponseMediaType::QUICK_TIME
    } else if extension.eq_ignore_ascii_case("woff") {
        ResponseMediaType::WOFF
    } else if extension.eq_ignore_ascii_case("woff2") {
        ResponseMediaType::WOFF2
    } else if extension.eq_ignore_ascii_case("ttf") {
        ResponseMediaType::TTF
    } else if extension.eq_ignore_ascii_case("otf") {
        ResponseMediaType::OTF
    } else if extension.eq_ignore_ascii_case("eot") {
        ResponseMediaType::EOT
    } else if extension.eq_ignore_ascii_case("txt") {
        ResponseMediaType::PLAIN_TEXT
    } else {
        ResponseMediaType::OCTET_STREAM
    }
}

fn matches_ignore_ascii_case(value: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
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
