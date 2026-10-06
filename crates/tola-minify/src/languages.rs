//! The minified languages a caller selects, and the language a source name selects.

use std::path::Path;

/// The languages a caller minifies.
///
/// `Default` selects none; a caller that minifies selects the languages it wants.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MinifiedLanguages {
    /// Stylesheets.
    pub css: bool,
    /// Classic scripts and modules.
    pub javascript: bool,
}

impl MinifiedLanguages {
    /// The languages a caller selects.
    pub const fn new(css: bool, javascript: bool) -> Self {
        Self { css, javascript }
    }

    /// Whether this language is among the minified ones.
    pub(crate) const fn includes(self, language: MinifyLanguage) -> bool {
        match language {
            MinifyLanguage::Css => self.css,
            MinifyLanguage::ClassicJavaScript | MinifyLanguage::ModuleJavaScript => self.javascript,
        }
    }
}

/// The language a source name selects.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MinifyLanguage {
    /// A stylesheet.
    Css,
    /// A classic script.
    ClassicJavaScript,
    /// A module.
    ModuleJavaScript,
}

impl MinifyLanguage {
    /// The language a source path selects by extension.
    ///
    /// `.css`, `.js`, and `.mjs` are recognized regardless of case; any other extension names none.
    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("css") {
            Some(Self::Css)
        } else if extension.eq_ignore_ascii_case("js") {
            Some(Self::ClassicJavaScript)
        } else if extension.eq_ignore_ascii_case("mjs") {
            Some(Self::ModuleJavaScript)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_names_its_language() {
        for (path, expected) in [
            ("assets/app.css", MinifyLanguage::Css),
            ("assets/app.CSS", MinifyLanguage::Css),
            ("assets/app.js", MinifyLanguage::ClassicJavaScript),
            ("assets/app.mjs", MinifyLanguage::ModuleJavaScript),
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
    fn switches_select_one_language_group() {
        let stylesheets_only = MinifiedLanguages::new(true, false);
        assert!(stylesheets_only.includes(MinifyLanguage::Css));
        assert!(!stylesheets_only.includes(MinifyLanguage::ClassicJavaScript));
        assert!(!stylesheets_only.includes(MinifyLanguage::ModuleJavaScript));

        let scripts_only = MinifiedLanguages::new(false, true);
        assert!(!scripts_only.includes(MinifyLanguage::Css));
        assert!(scripts_only.includes(MinifyLanguage::ClassicJavaScript));
        assert!(scripts_only.includes(MinifyLanguage::ModuleJavaScript));
    }
}
