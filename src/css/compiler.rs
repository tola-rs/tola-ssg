//! Atomic CSS compiler backend boundary.

use anyhow::Result;

use crate::css::config::AtomicCssFile;

/// Generate atomic CSS from source text.
pub fn compile<'a>(
    sources: impl IntoIterator<Item = &'a str>,
    config: &AtomicCssFile,
) -> Result<String> {
    let mut backend = config.backend_config()?;
    // Tola does not emit preflight unless `[preflight]` explicitly selects one.
    backend.preflight = encre_css::Preflight::new_none();

    if let Some(preflight) = &config.preflight {
        backend.preflight = crate::css::preflight::render(preflight)?;
    }

    Ok(encre_css::generate(sources, &backend))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::config::AtomicCssFile;

    #[test]
    fn compiler_generates_basic_atomic_utility() {
        let css = compile(["<div class=\"flex\"></div>"], &AtomicCssFile::default()).unwrap();

        assert!(css.contains(".flex"));
        assert!(css.contains("display"));
        assert!(css.contains("flex"));
    }

    #[test]
    fn compiler_uses_theme_tokens_from_atomic_config() {
        let config = AtomicCssFile::parse_str(
            r##"
[theme.colors]
primary = "#e5186a"
"##,
        )
        .unwrap();

        let css = compile(["<div class=\"bg-primary\"></div>"], &config).unwrap();

        assert!(css.contains(".bg-primary"));
        assert!(css.contains("#e5186a"));
    }

    #[test]
    fn compiler_scopes_profile_preflight_when_requested() {
        let config = AtomicCssFile::parse_str(
            r#"
[preflight]
use = { source = "profile", name = "tailwind-v4" }
scope = ".app"
"#,
        )
        .unwrap();

        let css = compile(std::iter::empty::<&str>(), &config).unwrap();

        assert!(css.contains("@scope (.app)"));
        assert!(css.contains("box-sizing: border-box"));
    }
}
