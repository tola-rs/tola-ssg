//! Minification state for configured static assets.
//!
//! Minification is an optimization: a source Tola cannot minify is published with its own bytes,
//! and the author reads a warning naming the source and what was wrong with it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tola_minify::{
    CssMinifyError, JavaScriptMinifyError, MinifiedLanguages, MinifyFailure, MinifyLanguage,
    MinifyRequest,
};

use crate::diagnostic::{Diagnostic, Severity};

/// Cache identity for bytes derived from one immutable raw observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AssetMinifyKey {
    digest: tola_typst::ContentDigest,
    request: MinifyRequest,
}

impl AssetMinifyKey {
    pub(crate) fn for_source(
        source_path: &Path,
        digest: tola_typst::ContentDigest,
        languages: MinifiedLanguages,
    ) -> Option<Self> {
        MinifyRequest::for_source(source_path, languages).map(|request| Self { digest, request })
    }
}

/// Minification state for one inventory render: the bytes it already produced and what it could
/// not minify.
///
/// The cache is shared by every source one render publishes, so equal bytes and language are
/// transformed once; the warnings name the sources that kept their own bytes.
#[derive(Default)]
pub(crate) struct AssetMinification {
    cache: AssetMinifyCache,
    warnings: Vec<AssetMinifyWarning>,
}

impl AssetMinification {
    /// Seed one source already rendered for a previous inventory.
    pub(crate) fn seed(&mut self, key: Option<AssetMinifyKey>, bytes: Arc<[u8]>) {
        self.cache.seed(key, bytes);
    }

    /// The bytes one source publishes, minifying them when the site asked for it.
    ///
    /// A source whose minification fails keeps its raw bytes and reports why, so an
    /// optimization that cannot run never decides whether the site can be built.
    pub(crate) fn rendered_bytes(
        &mut self,
        source_path: &Path,
        display_source: &str,
        digest: tola_typst::ContentDigest,
        raw: &Arc<[u8]>,
        languages: MinifiedLanguages,
    ) -> Arc<[u8]> {
        self.cache.rendered_bytes(
            source_path,
            display_source,
            digest,
            raw,
            languages,
            &mut self.warnings,
        )
    }

    /// The warnings this render produced, in the order the sources were rendered.
    pub(crate) fn take_warnings(&mut self) -> Vec<AssetMinifyWarning> {
        std::mem::take(&mut self.warnings)
    }
}

/// Minified bytes shared by asset mappings within one render; excludes raw passthroughs.
#[derive(Default)]
struct AssetMinifyCache {
    bytes: HashMap<AssetMinifyKey, Arc<[u8]>>,
}

impl AssetMinifyCache {
    fn seed(&mut self, key: Option<AssetMinifyKey>, bytes: Arc<[u8]>) {
        if let Some(key) = key {
            self.bytes.entry(key).or_insert(bytes);
        }
    }

    fn rendered_bytes(
        &mut self,
        source_path: &Path,
        display_source: &str,
        digest: tola_typst::ContentDigest,
        raw: &Arc<[u8]>,
        languages: MinifiedLanguages,
        warnings: &mut Vec<AssetMinifyWarning>,
    ) -> Arc<[u8]> {
        let Some(key) = AssetMinifyKey::for_source(source_path, digest, languages) else {
            return Arc::clone(raw);
        };
        if let Some(bytes) = self.bytes.get(&key) {
            return Arc::clone(bytes);
        }
        let language = key.request.language();
        let Ok(source) = std::str::from_utf8(raw) else {
            warnings.push(AssetMinifyWarning::NotUtf8 {
                path: display_source.to_owned(),
                language,
            });
            return Arc::clone(raw);
        };
        match key.request.minify(source) {
            Ok(minified) => {
                let bytes: Arc<[u8]> = minified.into_bytes().into();
                self.bytes.insert(key, Arc::clone(&bytes));
                bytes
            }
            Err(failure) => {
                warnings.push(AssetMinifyWarning::from_failure(
                    language,
                    display_source,
                    failure,
                ));
                Arc::clone(raw)
            }
        }
    }
}

/// What one source's minification could not do, as the site author reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AssetMinifyWarning {
    /// The source is not UTF-8 text.
    NotUtf8 {
        path: String,
        language: MinifyLanguage,
    },
    /// The source could not be parsed.
    Syntax {
        path: String,
        language: MinifyLanguage,
        reason: String,
        line: u32,
        column: u32,
    },
    /// A failure Tola cannot explain for this source.
    Unexpected {
        path: String,
        language: MinifyLanguage,
        reason: String,
    },
}

impl AssetMinifyWarning {
    /// The warning the author reads: the source keeps its raw bytes either way.
    pub(crate) fn diagnostic(&self) -> Diagnostic {
        let language = self.language();
        let (path, message) = match self {
            Self::NotUtf8 { path, .. } => (path, format!("`{path}` is not UTF-8 text")),
            Self::Syntax { path, .. } => (path, format!("`{path}` has a syntax error")),
            Self::Unexpected { path, .. } => (path, format!("Tola could not minify `{path}`")),
        };
        let diagnostic = Diagnostic::at_path(
            crate::codes::build::MINIFY,
            Severity::Warning,
            path,
            message,
        );
        match self {
            Self::NotUtf8 { .. } => diagnostic
                .with_note("Tola published it unchanged")
                .with_help(format!(
                    "Save the file as UTF-8, or turn off `{}`",
                    minify_key(language)
                )),
            Self::Syntax {
                reason,
                line,
                column,
                ..
            } => diagnostic
                .with_position(*line as usize, *column as usize)
                .with_note(reason.clone())
                .with_note("Tola published it unchanged")
                .with_help(format!(
                    "Fix the file, or turn off `{}`",
                    minify_key(language)
                )),
            Self::Unexpected { reason, .. } => diagnostic
                .with_note(reason.clone())
                .with_note("Tola published it unchanged")
                .with_help(format!(
                    "Report this at {REPORT_URL} with the file, or turn off `{}`",
                    minify_key(language)
                )),
        }
    }

    /// The site-relative source this warning names.
    pub(crate) fn path(&self) -> &str {
        match self {
            Self::NotUtf8 { path, .. }
            | Self::Syntax { path, .. }
            | Self::Unexpected { path, .. } => path,
        }
    }

    fn language(&self) -> MinifyLanguage {
        match self {
            Self::NotUtf8 { language, .. }
            | Self::Syntax { language, .. }
            | Self::Unexpected { language, .. } => *language,
        }
    }

    fn from_failure(language: MinifyLanguage, path: &str, failure: MinifyFailure) -> Self {
        let path = path.to_owned();
        match failure {
            MinifyFailure::Css(CssMinifyError::Syntax {
                reason,
                line,
                column,
            }) => Self::Syntax {
                path,
                language,
                reason,
                line,
                column,
            },
            MinifyFailure::JavaScript(JavaScriptMinifyError::Syntax {
                reason,
                line,
                column,
            }) => Self::Syntax {
                path,
                language,
                reason,
                line,
                column,
            },
            // The semantic minifier and the printer report failures Tola's own options cannot
            // produce, so this arm exists to report one honestly rather than to expect it.
            MinifyFailure::Css(other) => Self::Unexpected {
                path,
                language,
                reason: other.to_string(),
            },
        }
    }
}

/// Where the author turns minification off for one language.
const fn minify_key(language: MinifyLanguage) -> &'static str {
    match language {
        MinifyLanguage::Css => "build.minify.css",
        MinifyLanguage::ClassicJavaScript | MinifyLanguage::ModuleJavaScript => {
            "build.minify.javascript"
        }
    }
}

const REPORT_URL: &str = "https://github.com/tola-rs/tola-ssg/issues";

#[cfg(test)]
mod tests {
    use super::*;

    fn all_asset_minification() -> MinifiedLanguages {
        MinifiedLanguages::new(true, true)
    }

    fn rendered(path: &str, source: &[u8]) -> (Arc<[u8]>, Vec<AssetMinifyWarning>) {
        let raw: Arc<[u8]> = Arc::from(source);
        let mut minification = AssetMinification::default();
        let bytes = minification.rendered_bytes(
            Path::new(path),
            path,
            tola_typst::ContentDigest::of(source),
            &raw,
            all_asset_minification(),
        );
        (bytes, minification.take_warnings())
    }

    #[test]
    fn raw_sources_keep_their_own_bytes() {
        for (path, languages) in [
            ("app.css", MinifiedLanguages::default()),
            ("asset.bin", all_asset_minification()),
            ("app.MIN.CSS", all_asset_minification()),
        ] {
            let raw: Arc<[u8]> = Arc::from(b"source bytes".as_slice());
            let mut minification = AssetMinification::default();
            let published = minification.rendered_bytes(
                Path::new(path),
                path,
                tola_typst::ContentDigest::of(&raw),
                &raw,
                languages,
            );
            assert!(Arc::ptr_eq(&raw, &published), "{path}");
            assert!(minification.take_warnings().is_empty(), "{path}");
        }
    }

    #[test]
    fn equal_bytes_share_one_transformation() {
        let source = b".a { color: red; }";
        let raw: Arc<[u8]> = Arc::from(source.as_slice());
        let digest = tola_typst::ContentDigest::of(source);
        let mut minification = AssetMinification::default();
        let first = minification.rendered_bytes(
            Path::new("first.css"),
            "first.css",
            digest,
            &raw,
            all_asset_minification(),
        );
        let second = minification.rendered_bytes(
            Path::new("second.css"),
            "second.css",
            digest,
            &raw,
            all_asset_minification(),
        );
        assert!(Arc::ptr_eq(&first, &second));
        assert!(minification.take_warnings().is_empty());
    }

    #[test]
    fn unminifiable_source_keeps_its_bytes() {
        let (bytes, warnings) = rendered("app.js", b"function =");
        assert_eq!(&*bytes, b"function =");
        let [warning] = &warnings[..] else {
            panic!("expected one warning, got {warnings:?}");
        };
        let diagnostic = warning.diagnostic();
        assert_eq!(diagnostic.code, "build.minify");
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.message, "`app.js` has a syntax error");
        assert_eq!(diagnostic.location.as_ref().unwrap().line, Some(1));
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note == "Tola published it unchanged")
        );
        assert!(
            diagnostic
                .help
                .iter()
                .any(|help| help.message.contains("build.minify.javascript")),
            "{:?}",
            diagnostic.help
        );
    }

    #[test]
    fn non_utf8_source_keeps_its_bytes() {
        let (bytes, warnings) = rendered("app.css", &[0xff, 0xfe]);
        assert_eq!(&*bytes, &[0xff, 0xfe]);
        let [warning] = &warnings[..] else {
            panic!("expected one warning, got {warnings:?}");
        };
        let diagnostic = warning.diagnostic();
        assert_eq!(diagnostic.message, "`app.css` is not UTF-8 text");
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note == "Tola published it unchanged")
        );
    }

    #[test]
    fn syntax_error_reports_its_position() {
        let (_, warnings) = rendered("app.css", b".a { color: red }\n@media (");
        let [warning] = &warnings[..] else {
            panic!("expected one warning, got {warnings:?}");
        };
        let AssetMinifyWarning::Syntax { line, column, .. } = warning else {
            panic!("expected a syntax warning, got {warning:?}");
        };
        assert_eq!((*line, *column), (2, 9));
    }
}
