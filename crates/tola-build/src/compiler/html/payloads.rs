//! CSS and JavaScript payloads inside generated HTML documents.

use tola_minify::{CssMinifyError, JavaScriptKind, JavaScriptMinifyError, MinifiedLanguages};
use tola_typst::{
    BundleCancellation, BundleCompilation, HtmlRawText, HtmlRawTextError, TypstWorld,
};
use typst::syntax::{Span, VirtualPath};

use crate::diagnostic::{Diagnostic, Location, Severity};

/// Minify the stylesheet and script payloads of one compiled Bundle.
///
/// A payload whose language is not minified, or that the minifier cannot rewrite, is kept
/// unchanged. One payload written in source is one mistake: its warning names the source that
/// wrote it and the documents it reached, however many of them hold it. Returns the number of
/// rewritten payloads.
pub(crate) fn minify_generated_payloads(
    compilation: &mut BundleCompilation,
    world: &TypstWorld,
    minified: MinifiedLanguages,
    diagnostics: &mut Vec<Diagnostic>,
    cancellation: &BundleCancellation,
) -> Result<usize, HtmlRawTextError> {
    let mut unminified = Vec::new();
    let rewritten =
        compilation.rewrite_html_raw_text(cancellation, |document, kind, span, text| {
            let enabled = match kind {
                HtmlRawText::Stylesheet => minified.css,
                HtmlRawText::Script | HtmlRawText::Module => minified.javascript,
            };
            if !enabled {
                return None;
            }
            let minified_payload = match kind {
                HtmlRawText::Stylesheet => {
                    tola_minify::minify_css(text).map_err(PayloadMinifyFailure::css)
                }
                HtmlRawText::Script => {
                    tola_minify::minify_javascript(text, JavaScriptKind::Classic)
                        .map_err(PayloadMinifyFailure::javascript)
                }
                HtmlRawText::Module => tola_minify::minify_javascript(text, JavaScriptKind::Module)
                    .map_err(PayloadMinifyFailure::javascript),
            };
            match minified_payload {
                Ok(minified) => Some(minified),
                Err(failure) => {
                    unminified.push(unminified_payload(world, document, span, kind, failure));
                    None
                }
            }
        })?;
    diagnostics.extend(crate::diagnostic::collapse_repeated(unminified));
    Ok(rewritten)
}

/// The identity of one payload written in source: its kind and the location that wrote it.
///
/// One payload that several documents hold is one mistake, so the author fixes one source position
/// instead of reading the same warning per page. A payload with no source location falls back to
/// the document with it, and two documents are two records.
#[derive(PartialEq, Eq, Hash)]
struct PayloadIdentity {
    kind: HtmlRawText,
    path: String,
    line: Option<usize>,
    column: Option<usize>,
}

/// The warning one unminified payload is reported with, its identity, and the document it reached.
fn unminified_payload(
    world: &TypstWorld,
    document: &VirtualPath,
    span: Span,
    kind: HtmlRawText,
    failure: PayloadMinifyFailure,
) -> (PayloadIdentity, Diagnostic, String) {
    let location = payload_location(world, document, span);
    let identity = PayloadIdentity {
        kind,
        path: location.path.clone(),
        line: location.line,
        column: location.column,
    };
    let (payload, switch) = match kind {
        HtmlRawText::Stylesheet => ("stylesheet", "build.minify.css"),
        HtmlRawText::Script | HtmlRawText::Module => ("script", "build.minify.javascript"),
    };
    let (reason, parser_position) = match failure {
        PayloadMinifyFailure::Syntax {
            reason,
            line,
            column,
        } => (reason, Some((line, column))),
        PayloadMinifyFailure::Unexpected { reason } => (reason, None),
    };
    let mut diagnostic = Diagnostic::at_location(
        crate::codes::build::MINIFY,
        Severity::Warning,
        location,
        format!("cannot minify the {payload} this document writes"),
    )
    .with_note(reason);
    if let Some((line, column)) = parser_position {
        diagnostic = diagnostic.with_note(format!(
            "the parser stopped at line {line}, column {column} of the {payload}"
        ));
    }
    let diagnostic = diagnostic
        .with_note(format!("the {payload} is kept unchanged"))
        .with_help(format!("Fix the {payload}, or turn off `{switch}`"));
    (
        identity,
        diagnostic,
        document.get_without_slash().to_owned(),
    )
}

/// Where one payload came from: the source that wrote it, or the document that has it.
///
/// A synthesized payload names no file, so its warning falls back to the document.
fn payload_location(world: &TypstWorld, document: &VirtualPath, span: Span) -> Location {
    match crate::compiler::source_location(world, span) {
        Some(location) => location,
        None => Location {
            path: document.get_without_slash().to_owned(),
            line: None,
            column: None,
            range: None,
            source_lines: Vec::new(),
        },
    }
}

/// What one payload's minification could not do, as the site author reads it.
enum PayloadMinifyFailure {
    /// The parser's own reason and one-based position inside the payload.
    Syntax {
        reason: String,
        line: u32,
        column: u32,
    },
    Unexpected {
        reason: String,
    },
}

impl PayloadMinifyFailure {
    fn css(error: CssMinifyError) -> Self {
        match error {
            CssMinifyError::Syntax {
                reason,
                line,
                column,
            } => Self::Syntax {
                reason,
                line,
                column,
            },
            // The semantic minifier and the printer report failures Tola's own options cannot
            // produce, so this arm exists to report one honestly rather than to expect it.
            other => Self::Unexpected {
                reason: other.to_string(),
            },
        }
    }

    fn javascript(error: JavaScriptMinifyError) -> Self {
        match error {
            JavaScriptMinifyError::Syntax {
                reason,
                line,
                column,
            } => Self::Syntax {
                reason,
                line,
                column,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use tola_typst::BundleOptions;

    const SITE: &str = r#"
#document("index.html")[
  #html.style(".a { color: red; } .b { color: red; }")
  #html.script("function answer() { return 6 * 7; }")
  #html.elem("script", attrs: (type: "module"))[export const answer = 6 + 6]
  #html.elem("script", attrs: (type: "application/json"))[#("{\"a\": 1, \"b\": 2}")]
  #html.elem("p", attrs: (style: "color: red;"))[Body]
]
"#;

    fn payloads(
        compilation: &mut BundleCompilation,
        cancellation: &BundleCancellation,
    ) -> BTreeMap<&'static str, String> {
        let mut found = BTreeMap::new();
        compilation
            .rewrite_html_raw_text(cancellation, |_, kind, _, text| {
                let key = match kind {
                    HtmlRawText::Stylesheet => "stylesheet",
                    HtmlRawText::Script => "script",
                    HtmlRawText::Module => "module",
                };
                found.insert(key, text.to_owned());
                None
            })
            .unwrap();
        found
    }

    fn exported_html(compilation: &BundleCompilation, cancellation: &BundleCancellation) -> String {
        let entries = compilation
            .export_entries(&BundleOptions::default(), cancellation, None)
            .unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.path().get_with_slash().ends_with("index.html"))
            .expect("the site publishes index.html");
        String::from_utf8(entry.bytes().to_vec()).unwrap()
    }

    #[test]
    fn generated_payloads_are_minified_in_place() {
        let (_directory, mut realized, cancellation) = crate::compiler::tests::realized_site(SITE);
        let before = payloads(&mut realized.compilation, &cancellation);
        let mut diagnostics = Vec::new();

        let rewritten = minify_generated_payloads(
            &mut realized.compilation,
            &realized.world,
            MinifiedLanguages::new(true, true),
            &mut diagnostics,
            &cancellation,
        )
        .unwrap();

        assert_eq!(rewritten, 3, "stylesheet, script, and module payloads");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let html = exported_html(&realized.compilation, &cancellation);
        assert!(
            html.contains(&tola_minify::minify_css(&before["stylesheet"]).unwrap()),
            "{html}"
        );
        assert!(
            html.contains(
                &tola_minify::minify_javascript(&before["script"], JavaScriptKind::Classic)
                    .unwrap()
            ),
            "{html}"
        );
        assert!(
            html.contains(
                &tola_minify::minify_javascript(&before["module"], JavaScriptKind::Module).unwrap()
            ),
            "{html}"
        );
        // A data block's payload is not source text, and attributes are never rewritten.
        assert!(html.contains(r#"{"a": 1, "b": 2}"#), "{html}");
        assert!(html.contains(r#"style="color: red;""#), "{html}");
    }

    #[test]
    fn disabled_language_keeps_its_payload() {
        let (_directory, mut realized, cancellation) = crate::compiler::tests::realized_site(SITE);
        let before = payloads(&mut realized.compilation, &cancellation);
        let mut diagnostics = Vec::new();

        let rewritten = minify_generated_payloads(
            &mut realized.compilation,
            &realized.world,
            MinifiedLanguages::new(true, false),
            &mut diagnostics,
            &cancellation,
        )
        .unwrap();

        assert_eq!(rewritten, 1, "only the stylesheet payload is minified");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let html = exported_html(&realized.compilation, &cancellation);
        assert!(html.contains(&before["script"]), "{html}");
        assert!(html.contains(&before["module"]), "{html}");
    }

    #[test]
    fn unparsable_payload_is_reported_once() {
        // One stylesheet written in source reaches both documents, so the author reads one warning
        // for the one source position they have to fix.
        let site = r#"
#let page(name) = document(name)[#html.style("@media (")]
#page("index.html")
#page("about.html")
"#;
        let (_directory, mut realized, cancellation) = crate::compiler::tests::realized_site(site);
        let before = payloads(&mut realized.compilation, &cancellation);
        let mut diagnostics = Vec::new();

        let rewritten = minify_generated_payloads(
            &mut realized.compilation,
            &realized.world,
            MinifiedLanguages::new(true, true),
            &mut diagnostics,
            &cancellation,
        )
        .unwrap();

        assert_eq!(rewritten, 0);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.code, crate::codes::build::MINIFY);
        assert_eq!(diagnostic.severity, Severity::Warning);
        let location = diagnostic.location.as_ref().expect("a located warning");
        assert_eq!(location.path, "site.typ");
        assert_eq!((location.line, location.column), (Some(2), Some(46)));
        assert!(
            !diagnostic.message.contains("index.html")
                && !diagnostic.message.contains("about.html"),
            "{diagnostic:?}"
        );
        // The parser's own reason and position survive for the one payload written in source.
        let CssMinifyError::Syntax { reason, .. } =
            tola_minify::minify_css("@media (").unwrap_err()
        else {
            panic!("the test stylesheet is unparsable");
        };
        assert!(
            diagnostic.notes.iter().any(|note| note == &reason),
            "{diagnostic:?}"
        );
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note.contains("line 1, column 9")),
            "{diagnostic:?}"
        );
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note.contains("`index.html`") && note.contains("`about.html`")),
            "{diagnostic:?}"
        );
        assert!(
            diagnostic
                .help
                .iter()
                .any(|help| help.message.contains("build.minify.css")),
            "{diagnostic:?}"
        );
        let html = exported_html(&realized.compilation, &cancellation);
        assert!(html.contains(&before["stylesheet"]), "{html}");
    }
}
