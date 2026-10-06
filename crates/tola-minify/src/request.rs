//! The minification one source name takes, and what it can fail with.

use std::path::Path;

use thiserror::Error;

use crate::{
    CssMinifyError, JavaScriptKind, JavaScriptMinifyError, MinifiedLanguages, MinifyLanguage,
    minify_css, minify_javascript,
};

const MINIFIED_SOURCE_SUFFIX: &str = ".min";

/// The minification one source name takes, independent of its bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MinifyRequest {
    language: MinifyLanguage,
}

impl MinifyRequest {
    /// The minification a source path takes.
    ///
    /// A source already named `*.min.*`, a source whose extension names no language, and a language
    /// the caller does not minify take no minification at all.
    pub fn for_source(path: &Path, languages: MinifiedLanguages) -> Option<Self> {
        if has_minified_suffix(path) {
            return None;
        }
        let language = MinifyLanguage::for_path(path)?;
        languages.includes(language).then_some(Self { language })
    }

    /// The language this request minifies.
    pub const fn language(self) -> MinifyLanguage {
        self.language
    }

    /// Minify one source text.
    pub fn minify(self, source: &str) -> Result<String, MinifyFailure> {
        match self.language {
            MinifyLanguage::Css => minify_css(source).map_err(MinifyFailure::Css),
            MinifyLanguage::ClassicJavaScript => minify_javascript(source, JavaScriptKind::Classic)
                .map_err(MinifyFailure::JavaScript),
            MinifyLanguage::ModuleJavaScript => {
                minify_javascript(source, JavaScriptKind::Module).map_err(MinifyFailure::JavaScript)
            }
        }
    }
}

/// What a source's minification could not rewrite.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum MinifyFailure {
    /// A stylesheet.
    #[error(transparent)]
    Css(CssMinifyError),
    /// A script or module.
    #[error(transparent)]
    JavaScript(JavaScriptMinifyError),
}

fn has_minified_suffix(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.get(stem.len().saturating_sub(MINIFIED_SOURCE_SUFFIX.len())..))
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(MINIFIED_SOURCE_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLESHEET: &str = "assets/app.css";
    const SCRIPT: &str = "assets/app.js";

    #[test]
    fn unminified_language_takes_no_request() {
        let stylesheets_only = MinifiedLanguages::new(true, false);
        assert_eq!(
            MinifyRequest::for_source(Path::new(STYLESHEET), stylesheets_only)
                .map(MinifyRequest::language),
            Some(MinifyLanguage::Css)
        );
        assert!(MinifyRequest::for_source(Path::new(SCRIPT), stylesheets_only).is_none());

        let scripts_only = MinifiedLanguages::new(false, true);
        assert!(MinifyRequest::for_source(Path::new(STYLESHEET), scripts_only).is_none());
        assert_eq!(
            MinifyRequest::for_source(Path::new("assets/app.mjs"), scripts_only)
                .map(MinifyRequest::language),
            Some(MinifyLanguage::ModuleJavaScript)
        );
    }

    #[test]
    fn minified_source_takes_no_request() {
        let languages = MinifiedLanguages::new(true, true);
        for path in [
            "assets/app.min.css",
            "assets/app.MIN.CSS",
            "assets/app.min.js",
        ] {
            assert!(
                MinifyRequest::for_source(Path::new(path), languages).is_none(),
                "{path}"
            );
        }
    }

    #[test]
    fn request_minifies_with_its_language() {
        let languages = MinifiedLanguages::new(true, true);
        let stylesheet = MinifyRequest::for_source(Path::new(STYLESHEET), languages).unwrap();
        assert_eq!(
            stylesheet.minify(".a { color: red; }").unwrap(),
            ".a{color:red}"
        );

        let script = MinifyRequest::for_source(Path::new(SCRIPT), languages).unwrap();
        assert!(matches!(
            script.minify("function =").unwrap_err(),
            MinifyFailure::JavaScript(JavaScriptMinifyError::Syntax { .. })
        ));

        assert!(matches!(
            stylesheet.minify("@media (").unwrap_err(),
            MinifyFailure::Css(CssMinifyError::Syntax { .. })
        ));
    }
}
