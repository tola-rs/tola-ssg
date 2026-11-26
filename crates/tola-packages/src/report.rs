//! Reports from Typst code retained in the tracked warning sink.

#![expect(
    clippy::too_many_arguments,
    reason = "`#[func]` clears attributes on the function it rewrites"
)]

use tola_typst::diagnostic::{DiagnosticOrigin, producer_warning};
use typst::diag::{HintedStrResult, SourceDiagnostic, SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{
    Array, CastInfo, Content, FromValue, IntoValue, Reflect, Smart, Str, Type, Value, func,
};
use typst::syntax::package::PackageSpec;
use typst::syntax::{Span, Spanned, VirtualRoot};

/// Producer origin name for `@tola/diagnostic` reports.
pub const REPORT_ORIGIN: &str = "TypstReport";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl ReportSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }

    fn parse(val: &str) -> Option<Self> {
        match val {
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            "hint" => Some(Self::Hint),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportCaller {
    Project,
    Package(PackageSpec),
    Unknown,
}

impl ReportCaller {
    fn from_span(span: Span) -> Self {
        let Some(id) = span.id() else {
            return Self::Unknown;
        };
        match id.root() {
            VirtualRoot::Project => Self::Project,
            VirtualRoot::Package(pkg) => Self::Package(pkg.clone()),
        }
    }

    fn encoded(&self) -> Option<String> {
        match self {
            Self::Project => Some("project".into()),
            Self::Package(pkg) => Some(pkg.to_string()),
            Self::Unknown => None,
        }
    }
}

/// Decoded report origin: the first `hint_count` hints are ordinary, the rest related.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOrigin {
    pub severity: ReportSeverity,
    pub code: Option<String>,
    pub caller: ReportCaller,
    pub hint_count: usize,
}

impl ReportOrigin {
    pub fn from_origin(origin: &DiagnosticOrigin) -> Option<Self> {
        let DiagnosticOrigin::Producer(origin) = origin else {
            return None;
        };
        if origin.name() != REPORT_ORIGIN {
            return None;
        }
        let mut severity = None;
        let mut code = None;
        let mut caller = None;
        let mut hint_count = None;
        let mut details = false;
        for (key, val) in origin.fields() {
            match key.as_str() {
                "severity" if severity.is_none() => severity = Some(ReportSeverity::parse(val)?),
                "code" if code.is_none() && !val.is_empty() => code = Some(val.to_string()),
                "caller" if caller.is_none() => {
                    caller = Some(if val == "project" {
                        ReportCaller::Project
                    } else {
                        ReportCaller::Package(val.parse().ok()?)
                    });
                }
                "hints" if hint_count.is_none() => hint_count = Some(val.parse().ok()?),
                "details"
                    if !details
                        && val.len() == 32
                        && val.bytes().all(|b| b.is_ascii_hexdigit()) =>
                {
                    details = true;
                }
                _ => return None,
            }
        }
        Some(Self {
            severity: severity?,
            code,
            caller: caller.unwrap_or(ReportCaller::Unknown),
            hint_count: hint_count?,
        })
    }
}

/// Strict `content` wrapper that rejects string coercion (strings lack a source span).
#[derive(Debug, Clone)]
pub(super) struct ReportContent(Content);

impl Reflect for ReportContent {
    fn input() -> CastInfo {
        CastInfo::Type(Type::of::<Content>())
    }

    fn output() -> CastInfo {
        Self::input()
    }

    fn castable(val: &Value) -> bool {
        matches!(val, Value::Content(_))
    }
}

impl FromValue for ReportContent {
    fn from_value(val: Value) -> HintedStrResult<Self> {
        match val {
            Value::Content(c) => Ok(Self(c)),
            other => Err(Self::error(&other)),
        }
    }
}

impl IntoValue for ReportContent {
    fn into_value(self) -> Value {
        Value::Content(self.0)
    }
}

#[func]
pub(super) fn report(
    engine: &mut Engine,
    span: Span,
    /// The explanation of the reported issue.
    message: Str,
    /// The issue level: `"error"`, `"warning"`, `"info"`, or `"hint"`.
    #[named]
    #[default(Spanned::detached(Str::from("warning")))]
    severity: Spanned<Str>,
    /// An optional rule code owned by the calling project or package.
    #[named]
    #[default(Spanned::detached(None))]
    code: Spanned<Option<Str>>,
    /// `auto` uses this call; content uses its own native span; `none` has no location.
    #[named]
    #[default(Smart::Auto)]
    at: Smart<Option<ReportContent>>,
    /// An array of explanatory strings.
    #[named]
    #[default(Spanned::detached(Array::new()))]
    hints: Spanned<Array>,
    /// An array of `(message: str, at: content | none)` dictionaries.
    #[named]
    #[default(Spanned::detached(Array::new()))]
    related: Spanned<Array>,
) -> SourceResult<()> {
    let Some(severity) = ReportSeverity::parse(severity.v.as_str()) else {
        bail!(
            severity.span,
            "severity must be error, warning, info, or hint"
        );
    };
    if code.v.as_ref().is_some_and(|c| c.is_empty()) {
        bail!(code.span, "a diagnostic code must not be empty");
    }
    let mut notes = SourceDiagnostic::warning(Span::detached(), message.as_str());
    for hint in &hints.v {
        let Value::Str(hint) = hint else {
            bail!(hints.span, "each diagnostic hint must be a string");
        };
        notes.hint(hint.as_str());
    }
    for entry in &related.v {
        append_related(&mut notes, entry, related.span)?;
    }
    engine.sink.warn(diagnostic(
        span,
        notes,
        severity,
        code.v.as_ref().map(Str::as_str),
        at.map(|c| c.map(|c| c.0)),
        hints.v.len(),
    ));
    Ok(())
}

fn diagnostic(
    caller: Span,
    notes: SourceDiagnostic,
    severity: ReportSeverity,
    code: Option<&str>,
    at: Smart<Option<Content>>,
    hint_count: usize,
) -> SourceDiagnostic {
    let target = match at {
        Smart::Auto => caller,
        Smart::Custom(Some(content)) => content.span(),
        Smart::Custom(None) => Span::detached(),
    };
    let caller = ReportCaller::from_span(caller).encoded();
    let hint_count = hint_count.to_string();
    // Typst's sink deduplicates by span and message alone; `details` preserves distinct hint sets.
    let details =
        (!notes.hints.is_empty()).then(|| format!("{:032x}", typst::utils::hash128(&notes.hints)));
    let mut fields = vec![("severity", severity.as_str()), ("hints", &hint_count)];
    if let Some(code) = code {
        fields.push(("code", code));
    }
    if let Some(caller) = &caller {
        fields.push(("caller", caller));
    }
    if let Some(details) = &details {
        fields.push(("details", details));
    }
    // Reports travel as warnings so evaluation continues; the host restores requested severity.
    let mut diag = producer_warning(target, REPORT_ORIGIN, &fields, notes.message);
    diag.hints = notes.hints;
    diag
}

fn append_related(diag: &mut SourceDiagnostic, related: &Value, span: Span) -> SourceResult<()> {
    let Value::Dict(dict) = related else {
        bail!(
            span,
            "each related diagnostic must be a dictionary with message and at"
        );
    };
    if dict.len() != 2 {
        bail!(
            span,
            "a related diagnostic must contain only message and at"
        );
    }
    let Some(msg) = crate::dict_str(dict, "message") else {
        bail!(span, "a related diagnostic needs a string message");
    };
    let target = match dict.get("at") {
        Ok(Value::Content(content)) => content.span(),
        Ok(Value::None) => Span::detached(),
        _ => bail!(span, "a related diagnostic needs content or none for at"),
    };
    diag.spanned_hint(msg, target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use tola_typst::diagnostic::{Diagnostics, NativeDiagnostic};
    use tola_typst::{TypstWorld, compile_world, file_id, scan_world_observed};
    use typst::engine::Sink;
    use typst::syntax::{DiagSpan, FileId, RootedPath, Source, VirtualPath};

    use super::*;
    use crate::TolaPackage;
    use crate::library::SiteLibrary;
    use crate::test_support::{empty_inputs, file_map, test_world};

    const IMPORT: &str = "#import \"@tola/diagnostic:0.0.0\": report\n";

    fn span(root: VirtualRoot, path: &str) -> Span {
        let id = FileId::new(RootedPath::new(root, VirtualPath::new(path).unwrap()));
        Source::new(id, "sample".into()).root().span()
    }

    fn warning(caller: Span, target: Span, code: &str) -> SourceDiagnostic {
        diagnostic(
            caller,
            SourceDiagnostic::warning(Span::detached(), "a reported issue"),
            ReportSeverity::Warning,
            Some(code),
            Smart::Custom(Some(Content::empty().spanned(target))),
            0,
        )
    }

    fn origin(diag: &NativeDiagnostic) -> ReportOrigin {
        ReportOrigin::from_origin(diag.origin()).expect("typed public report")
    }

    fn world(files: &[(&str, &str)]) -> TypstWorld {
        let lib = SiteLibrary::new(empty_inputs()).shared();
        test_world("tola-public-report", file_map(files), lib)
    }

    fn scan(world: &TypstWorld) -> Diagnostics {
        scan_world_observed(world, [])
            .result()
            .unwrap_or_else(|err| panic!("report scan failed: {err:?}"))
            .diagnostics()
            .clone()
    }

    #[test]
    fn report_target_and_caller() {
        let caller = span(
            VirtualRoot::Package("@tola-modules/catalog:0.0.0".parse().unwrap()),
            "helpers/report.typ",
        );
        let target = span(VirtualRoot::Project, "article.typ");
        let native = NativeDiagnostic::from(warning(caller, target, "a-dynamic-rule/中文"));
        let orig = origin(&native);
        assert_eq!(
            orig.caller,
            ReportCaller::Package("@tola-modules/catalog:0.0.0".parse().unwrap())
        );
        assert_eq!(orig.code.as_deref(), Some("a-dynamic-rule/中文"));
        assert_eq!(native.source().span, DiagSpan::from(target));
    }

    #[test]
    fn report_at_variants() {
        let caller = span(VirtualRoot::Project, "main.typ");
        for (at, expected) in [
            (Smart::Auto, caller),
            (Smart::Custom(None), Span::detached()),
            (Smart::Custom(Some(Content::empty())), Span::detached()),
        ] {
            let native = NativeDiagnostic::from(diagnostic(
                caller,
                SourceDiagnostic::warning(Span::detached(), "issue"),
                ReportSeverity::Info,
                None,
                at,
                0,
            ));
            assert_eq!(native.source().span, DiagSpan::from(expected));
            assert_eq!(origin(&native).caller, ReportCaller::Project);
        }
        let unknown = NativeDiagnostic::from(warning(
            Span::detached(),
            Span::detached(),
            "unknown-caller",
        ));
        assert_eq!(origin(&unknown).caller, ReportCaller::Unknown);
    }

    #[test]
    fn report_sink_dedup() {
        let first = span(VirtualRoot::Project, "first.typ");
        let second = span(VirtualRoot::Project, "second.typ");
        let pkg = span(
            VirtualRoot::Package("@tola-modules/other:0.0.0".parse().unwrap()),
            "lib.typ",
        );
        let mut sink = Sink::new();
        sink.warn(warning(first, first, "one"));
        sink.warn(warning(first, first, "one"));
        sink.warn(warning(first, first, "two"));
        sink.warn(warning(first, second, "one"));
        sink.warn(warning(pkg, first, "one"));
        sink.warn(diagnostic(
            first,
            SourceDiagnostic::warning(Span::detached(), "a reported issue"),
            ReportSeverity::Error,
            Some("one"),
            Smart::Custom(Some(Content::empty().spanned(first))),
            0,
        ));
        assert_eq!(sink.warnings().len(), 5);
    }

    #[test]
    fn report_related_survives_dedup() {
        let caller = span(VirtualRoot::Project, "main.typ");
        let mut sink = Sink::new();
        for msg in ["first context", "second context", "first context"] {
            let notes = SourceDiagnostic::warning(Span::detached(), "same issue")
                .with_spanned_hint(msg, Span::detached());
            sink.warn(diagnostic(
                caller,
                notes,
                ReportSeverity::Warning,
                Some("same-rule"),
                Smart::Auto,
                0,
            ));
        }
        let reports = sink.warnings();
        assert_eq!(reports.len(), 2);
        let native: Vec<_> = reports.into_iter().map(NativeDiagnostic::from).collect();
        assert_eq!(origin(&native[0]), origin(&native[1]));
        assert_ne!(native[0].source().hints, native[1].source().hints);
    }

    #[test]
    fn report_import_span() {
        let text = format!("{IMPORT}#report(\"a cached issue\", code: \"cached\")");
        let w = world(&[("main.typ", &text)]);
        let cold = scan(&w);
        let warm = scan(&w);
        assert_eq!(cold.raw(), warm.raw());
        assert_eq!(cold.len(), 1);
        let native = &cold.raw()[0];
        assert_eq!(native.source().span.id(), Some(file_id("main.typ")));
        assert_eq!(origin(native).severity, ReportSeverity::Warning);
        assert_eq!(cold.iter().next().unwrap().location.line, Some(2));
    }

    #[test]
    fn report_target_spans() {
        let text = format!(
            "{IMPORT}#import \"authored.typ\": value\n#report(\"review\", at: value, related: ((message: \"original\", at: value),))"
        );
        let diags = scan(&world(&[
            ("main.typ", &text),
            ("authored.typ", "#let value = emph[authored]"),
        ]));
        assert_eq!(diags.len(), 1);
        let native = &diags.raw()[0];
        assert_eq!(native.source().span.id(), Some(file_id("authored.typ")));
        assert_eq!(
            native.source().hints[0].span.id(),
            Some(file_id("authored.typ"))
        );
        assert_eq!(origin(native).caller, ReportCaller::Project);
        assert_eq!(origin(native).hint_count, 0);
    }

    #[test]
    fn report_severities_and_hints() {
        let text = format!(
            "{IMPORT}{}",
            r#"
#report("first", severity: "error", code: "first", at: none,
  hints: ("A correction.",),
  related: ((message: "Context without a location.", at: none),))
#report("second", severity: "info", code: "second")
#report("third", severity: "hint", code: "third")
"#
        );
        let diags = scan(&world(&[("main.typ", &text)]));
        let reports: Vec<_> = diags.raw().iter().map(origin).collect();
        assert_eq!(reports.len(), 3, "error report did not stop evaluation");
        assert_eq!(reports[0].severity, ReportSeverity::Error);
        assert_eq!(reports[1].severity, ReportSeverity::Info);
        assert_eq!(reports[2].severity, ReportSeverity::Hint);
        assert_eq!(reports[0].hint_count, 1);
        let native = diags.raw()[0].source();
        assert_eq!(native.severity, typst::diag::Severity::Warning);
        assert!(native.span.is_detached());
        assert_eq!(native.hints.len(), 2);
        assert!(native.hints[1].span.is_detached());
        assert!(diags.iter().next().unwrap().location.path.is_none());
    }

    #[test]
    fn report_deferred_context() {
        let text = format!(
            "{IMPORT}#context {{ for _ in range(2) {{ report(\"realized\", code: \"deferred\", at: none) }} }}"
        );
        let w = world(&[("main.typ", &text)]);
        assert!(scan(&w).is_empty(), "context not realized in scan");
        let cold = compile_world(&w).unwrap();
        let warm = compile_world(&w).unwrap();
        assert_eq!(cold.diagnostics().raw(), warm.diagnostics().raw());
        let reports: Vec<_> = cold
            .diagnostics()
            .raw()
            .iter()
            .filter(|n| ReportOrigin::from_origin(n.origin()).is_some())
            .collect();
        assert_eq!(reports.len(), 1, "{:?}", cold.diagnostics().raw());
        assert_eq!(origin(reports[0]).code.as_deref(), Some("deferred"));
        let preset = tola_typst::diagnostic::DiagnosticFilter::html_export_warning_filter();
        for diag in cold.diagnostics().raw() {
            if ReportOrigin::from_origin(diag.origin()).is_none() {
                assert!(preset.matches(diag), "unexpected diagnostic: {diag:?}");
            }
        }
    }

    #[test]
    fn report_unsaved_line_offset() {
        let main = "#include \"first.typ\"\n#include \"second.typ\"";
        let first = format!("{IMPORT}#report(\"same issue\", code: \"same-rule\")");
        let second =
            format!("{IMPORT}// unsaved Unicode: 🦊\n#report(\"same issue\", code: \"same-rule\")");
        let orig = scan(&world(&[
            ("main.typ", main),
            ("first.typ", &first),
            ("second.typ", &first),
        ]));
        let repl = scan(&world(&[
            ("main.typ", main),
            ("first.typ", &first),
            ("second.typ", &second),
        ]));
        assert_eq!(orig.len(), 2);
        assert_eq!(repl.len(), 2);
        let loc = repl
            .iter()
            .find(|d| {
                d.location
                    .path
                    .as_deref()
                    .is_some_and(|p| p.ends_with("second.typ"))
            })
            .unwrap();
        assert_eq!(loc.location.line, Some(3));
        assert!(
            loc.location
                .source_lines
                .iter()
                .any(|l| l.text.contains("report"))
        );
    }

    #[test]
    fn report_rejects_invalid_args() {
        for args in [
            "at: \"article.typ\"",
            "at: (path: \"article.typ\", line: 1)",
            "owner: \"core\"",
            "cause: \"PackageArgument\"",
            "severity: \"fatal\"",
            "code: \"\"",
            "hints: (1,)",
            "related: ((message: \"x\", at: \"article.typ\"),)",
            "related: ((message: \"x\", at: none, cause: \"core\"),)",
        ] {
            let text = format!("{IMPORT}#report(\"issue\", {args})");
            let w = world(&[("main.typ", &text)]);
            assert!(
                scan_world_observed(&w, []).result().is_err(),
                "accepted {args}"
            );
        }
    }

    #[test]
    fn report_origin_rejects_invalid() {
        for (name, fields) in [
            (
                "PackageArgument",
                vec![("severity", "error"), ("hints", "0")],
            ),
            (REPORT_ORIGIN, vec![("severity", "warning")]),
            (REPORT_ORIGIN, vec![("severity", "fatal"), ("hints", "0")]),
            (
                REPORT_ORIGIN,
                vec![("severity", "warning"), ("hints", "-1")],
            ),
            (
                REPORT_ORIGIN,
                vec![("severity", "warning"), ("hints", "0"), ("code", "")],
            ),
            (
                REPORT_ORIGIN,
                vec![
                    ("severity", "warning"),
                    ("hints", "0"),
                    ("caller", "article.typ"),
                ],
            ),
            (
                REPORT_ORIGIN,
                vec![("severity", "warning"), ("hints", "0"), ("cause", "core")],
            ),
            (
                REPORT_ORIGIN,
                vec![("severity", "warning"), ("hints", "0"), ("hints", "1")],
            ),
        ] {
            let native =
                NativeDiagnostic::from(producer_warning(Span::detached(), name, &fields, "x"));
            assert!(
                ReportOrigin::from_origin(native.origin()).is_none(),
                "accepted {fields:?}"
            );
        }
    }

    #[test]
    fn report_docs_signature() {
        let pkg = crate::builtin_package(&TolaPackage::Diagnostic.spec()).unwrap();
        assert_eq!(pkg.exports().collect::<Vec<_>>(), ["report"]);
        let exports = pkg.export_documentation(&["report".into()]).unwrap();
        let crate::ExportDeclaration::Function(sig) = &exports[0].declaration else {
            panic!("report is a native function");
        };
        assert_eq!(sig.parameters.len(), 6);
        let at = sig.parameters.iter().find(|p| p.name == "at").unwrap();
        assert_eq!(at.default.as_deref(), Some("auto"));
        assert!(at.ty_spelling().contains("content"));
    }
}
