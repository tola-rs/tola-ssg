//! Language selection and unified minification entrypoints.

use std::path::Path;

use thiserror::Error;

use crate::{CssMinifyError, JsKind, JsMinifyError, minify_css, minify_js};

const MINIFIED_SUFFIX: &[u8] = b".min";

/// Languages enabled for minification; `Default` disables all languages.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MinifiedLanguages {
    /// Minify `.css` stylesheets.
    pub css: bool,
    /// Minify `.js` and `.mjs` scripts.
    pub javascript: bool,
}

impl MinifiedLanguages {
    /// Construct a language selection from per-language flags.
    pub const fn new(css: bool, javascript: bool) -> Self {
        Self { css, javascript }
    }

    /// Return whether `lang` is enabled in this selection.
    pub const fn includes(self, lang: MinifyLanguage) -> bool {
        match lang {
            MinifyLanguage::Css => self.css,
            MinifyLanguage::ClassicJs | MinifyLanguage::ModuleJs => self.javascript,
        }
    }
}

/// Minifiable source language identified by path extension.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MinifyLanguage {
    /// CSS stylesheet.
    Css,
    /// Classic JavaScript script.
    ClassicJs,
    /// ES module JavaScript script.
    ModuleJs,
}

impl MinifyLanguage {
    /// Identify the minifiable language from a path's extension (`.css`, `.js`, `.mjs`).
    pub fn for_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        if ext.eq_ignore_ascii_case("css") {
            Some(Self::Css)
        } else if ext.eq_ignore_ascii_case("js") {
            Some(Self::ClassicJs)
        } else if ext.eq_ignore_ascii_case("mjs") {
            Some(Self::ModuleJs)
        } else {
            None
        }
    }

    /// Select the minification language for `path` if enabled and not already `*.min.*`.
    pub fn for_source(path: &Path, enabled: MinifiedLanguages) -> Option<Self> {
        if has_minified_suffix(path) {
            return None;
        }
        let lang = Self::for_path(path)?;
        enabled.includes(lang).then_some(lang)
    }

    /// Minify `src` using this language's rules.
    pub fn minify(self, src: &str) -> Result<String, MinifyError> {
        match self {
            Self::Css => minify_css(src).map_err(MinifyError::Css),
            Self::ClassicJs => minify_js(src, JsKind::Classic).map_err(MinifyError::Js),
            Self::ModuleJs => minify_js(src, JsKind::Module).map_err(MinifyError::Js),
        }
    }
}

/// Error returned when a source cannot be minified.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum MinifyError {
    /// Stylesheet minification failed.
    #[error(transparent)]
    Css(CssMinifyError),
    /// JavaScript minification failed.
    #[error(transparent)]
    Js(JsMinifyError),
}

fn has_minified_suffix(path: &Path) -> bool {
    let Some(stem) = path.file_stem() else {
        return false;
    };
    let bytes = stem.as_encoded_bytes();
    bytes
        .get(bytes.len().saturating_sub(MINIFIED_SUFFIX.len())..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(MINIFIED_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_language() {
        for (path, expected) in [
            ("assets/app.css", MinifyLanguage::Css),
            ("assets/app.CSS", MinifyLanguage::Css),
            ("assets/app.js", MinifyLanguage::ClassicJs),
            ("assets/app.mjs", MinifyLanguage::ModuleJs),
        ] {
            assert_eq!(
                MinifyLanguage::for_path(Path::new(path)),
                Some(expected),
                "{path}"
            );
        }
        for path in ["assets/app.bin", "assets/app", "assets/app.css.bak"] {
            assert_eq!(MinifyLanguage::for_path(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn source_selection_and_minify() {
        let all = MinifiedLanguages::new(true, true);
        assert_eq!(
            MinifyLanguage::for_source(Path::new("assets/app.min.css"), all),
            None
        );
        assert_eq!(
            MinifyLanguage::for_source(Path::new("assets/app.MIN.JS"), all),
            None
        );

        let css = MinifyLanguage::for_source(Path::new("assets/app.css"), all).unwrap();
        assert_eq!(css.minify(".a { color: red; }").unwrap(), ".a{color:red}");
        assert!(matches!(
            css.minify("@media (").unwrap_err(),
            MinifyError::Css(CssMinifyError::Syntax { .. })
        ));

        let js = MinifyLanguage::for_source(Path::new("assets/app.js"), all).unwrap();
        assert!(matches!(
            js.minify("function =").unwrap_err(),
            MinifyError::Js(JsMinifyError::Syntax { .. })
        ));
    }
}
