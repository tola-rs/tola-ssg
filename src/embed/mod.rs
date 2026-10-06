//! Embedded JavaScript and CSS for development responses.

pub mod dev {
    use std::sync::LazyLock;

    use tola_address::{RESERVED_ROOT, SiteUrlMount, UrlPath};

    const HOTRELOAD_FILENAME: &str = "hotreload.js";
    const HOTRELOAD_SOURCE: &str = include_str!("dev/hotreload.js");
    const DEV_STATUS_CSS: &str = include_str!("dev/hotreload-status.css");

    fn escape_template_literal(input: &str) -> String {
        input
            .replace('\\', "\\\\")
            .replace('`', "\\`")
            .replace("${", "\\${")
    }

    /// Why Tola could not minify its own development runtime.
    ///
    /// The failing step and the minifier's own message stay in Tola's log; the HTTP response
    /// has only the failure sentence a site author can act on.
    #[derive(Debug)]
    pub(crate) enum RuntimeMinifyError {
        /// The status stylesheet embedded in the runtime could not be minified.
        StatusStylesheet(String),
        /// The composed runtime could not be minified.
        RuntimeSource(String),
    }

    impl std::fmt::Display for RuntimeMinifyError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::StatusStylesheet(reason) => write!(f, "status stylesheet: {reason}"),
                Self::RuntimeSource(reason) => write!(f, "runtime source: {reason}"),
            }
        }
    }

    /// Compose the development runtime from Tola's own sources and minify it.
    ///
    /// The status stylesheet is minified before it is embedded: a JavaScript minifier cannot see
    /// inside the template literal that has it, so minifying the composed runtime alone would
    /// publish the stylesheet uncompressed.
    fn render_hotreload_js(source: &str, status_css: &str) -> Result<String, RuntimeMinifyError> {
        let status_css = tola_minify::minify_css(status_css)
            .map_err(|error| RuntimeMinifyError::StatusStylesheet(error.to_string()))?;
        let composed = source.replace(
            "__TOLA_DEV_STATUS_CSS__",
            &escape_template_literal(&status_css),
        );
        tola_minify::minify_javascript(&composed, tola_minify::JavaScriptKind::Classic)
            .map_err(|error| RuntimeMinifyError::RuntimeSource(error.to_string()))
    }

    pub(crate) fn hotreload_browser_path(mount: &SiteUrlMount) -> String {
        let path = UrlPath::from_decoded(&format!("/{RESERVED_ROOT}/{HOTRELOAD_FILENAME}"))
            .expect("the development runtime has a fixed portable URL path");
        mount.browser_path(&path)
    }

    pub(crate) fn hotreload_script_tag(mount: &SiteUrlMount, bootstrap: &str) -> String {
        let source = hotreload_browser_path(mount);
        format!(
            r#"<script src="{}" data-tola-runtime data-tola-bootstrap="{}"></script>"#,
            tola_build::html::escape_attr(&source),
            tola_build::html::escape_attr(bootstrap),
        )
    }

    /// The rendered development runtime every HTTP response serves, minified once per process.
    ///
    /// An unminifiable source is refused rather than published raw; the failure is logged once,
    /// where the runtime is prepared.
    static RENDERED_RUNTIME: LazyLock<Result<String, RuntimeMinifyError>> = LazyLock::new(|| {
        let rendered = render_hotreload_js(HOTRELOAD_SOURCE, DEV_STATUS_CSS);
        if let Err(failure) = &rendered {
            tracing::error!(
                target: "tola::dev",
                %failure,
                "the development runtime could not be minified"
            );
        }
        rendered
    });

    pub(crate) fn hotreload_js() -> Result<&'static str, &'static RuntimeMinifyError> {
        match &*RENDERED_RUNTIME {
            Ok(rendered) => Ok(rendered.as_str()),
            Err(failure) => Err(failure),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{hotreload_browser_path, hotreload_js, render_hotreload_js};

        #[test]
        fn runtime_path_follows_the_site_mount() {
            assert_eq!(
                hotreload_browser_path(&tola_address::SiteUrlMount::root()),
                "/_tola/hotreload.js"
            );
            let mount = tola_address::SiteUrlMount::from_base_path("/docs/blog/").unwrap();
            assert_eq!(
                hotreload_browser_path(&mount),
                "/docs/blog/_tola/hotreload.js"
            );
        }

        #[test]
        fn rendered_runtime_parses_as_javascript() {
            use oxc::allocator::Allocator;
            use oxc::parser::Parser;
            use oxc::span::SourceType;

            let source = hotreload_js().expect("Tola's own runtime sources minify");
            let allocator = Allocator::default();
            let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
            assert!(
                parsed.diagnostics.is_empty(),
                "{}",
                parsed
                    .diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }

        #[test]
        fn unminifiable_runtime_is_refused() {
            for (source, status_css) in [
                ("function =", "#tola-dev-status{color:red}"),
                ("const runtime = 1;", "@media ("),
            ] {
                assert!(
                    render_hotreload_js(source, status_css).is_err(),
                    "{source:?} / {status_css:?}"
                );
            }
        }
    }
}
