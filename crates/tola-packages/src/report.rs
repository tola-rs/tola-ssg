//! Reports from code already running in Typst, retained by its tracked warning sink.

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

/// The producer identity of a public `@tola/diagnostic` report.
pub const REPORT_ORIGIN: &str = "TypstReport";

/// The severity requested by the reporter, independently of Typst's warning transport.
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

    fn parse(value: &str) -> Option<Self> {
        match value {
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            "hint" => Some(Self::Hint),
            _ => None,
        }
    }
}

/// The logical caller recovered from the native call's file identity, never from an argument.
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
            VirtualRoot::Package(package) => Self::Package(package.clone()),
        }
    }

    fn encoded(&self) -> Option<String> {
        match self {
            Self::Project => Some("project".into()),
            Self::Package(package) => Some(package.to_string()),
            Self::Unknown => None,
        }
    }
}

/// Typed evidence of a retained report: the first `hint_count` hints are ordinary, the rest related.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOrigin {
    pub severity: ReportSeverity,
    pub code: Option<String>,
    pub caller: ReportCaller,
    pub hint_count: usize,
}

impl ReportOrigin {
    /// Decode only the complete, recognized producer contract.
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
        for (key, value) in origin.fields() {
            match key.as_str() {
                "severity" if severity.is_none() => severity = Some(ReportSeverity::parse(value)?),
                "code" if code.is_none() && !value.is_empty() => code = Some(value.to_string()),
                "caller" if caller.is_none() => {
                    caller = Some(if value == "project" {
                        ReportCaller::Project
                    } else {
                        ReportCaller::Package(value.parse().ok()?)
                    });
                }
                "hints" if hint_count.is_none() => hint_count = Some(value.parse().ok()?),
                "details"
                    if !details
                        && value.len() == 32
                        && value.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
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

/// Source evidence must already be content. Typst's ordinary `Content` cast also accepts a
/// string and creates new display content, which cannot stand for an authored source target.
#[derive(Debug, Clone)]
pub(super) struct ReportContent(Content);

impl Reflect for ReportContent {
    fn input() -> CastInfo {
        CastInfo::Type(Type::of::<Content>())
    }

    fn output() -> CastInfo {
        Self::input()
    }

    fn castable(value: &Value) -> bool {
        matches!(value, Value::Content(_))
    }
}

impl FromValue for ReportContent {
    fn from_value(value: Value) -> HintedStrResult<Self> {
        match value {
            Value::Content(content) => Ok(Self(content)),
            other => Err(Self::error(&other)),
        }
    }
}

impl IntoValue for ReportContent {
    fn into_value(self) -> Value {
        Value::Content(self.0)
    }
}

/// The native itself is re-exported, so `span` is the author's actual call.
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
    if code.v.as_ref().is_some_and(|code| code.is_empty()) {
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
        at.map(|content| content.map(|content| content.0)),
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
    // Sink deduplicates by span and message alone. Retain distinct explanations until the host
    // can merge the issue and all its related information by the semantic issue identity.
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
    // All requested levels travel as warnings. The host restores their requested severity and
    // decides whether a collected error prevents success, without aborting Typst evaluation.
    let mut diagnostic = producer_warning(target, REPORT_ORIGIN, &fields, notes.message);
    diagnostic.hints = notes.hints;
    diagnostic
}

fn append_related(
    diagnostic: &mut SourceDiagnostic,
    related: &Value,
    span: Span,
) -> SourceResult<()> {
    let Value::Dict(related) = related else {
        bail!(
            span,
            "each related diagnostic must be a dictionary with message and at"
        );
    };
    if related.len() != 2 {
        bail!(
            span,
            "a related diagnostic must contain only message and at"
        );
    }
    let Ok(Value::Str(message)) = related.get("message") else {
        bail!(span, "a related diagnostic needs a string message");
    };
    let target = match related.get("at") {
        Ok(Value::Content(content)) => content.span(),
        Ok(Value::None) => Span::detached(),
        _ => bail!(span, "a related diagnostic needs content or none for at"),
    };
    diagnostic.spanned_hint(message.as_str(), target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use tola_typst::diagnostic::{Diagnostics, NativeDiagnostic};
    use tola_typst::{
        FileMap, FileProvider, FileResolver, FileTarget, TypstWorld, compile_world, file_id,
        scan_world_observed,
    };
    use typst::engine::Sink;
    use typst::foundations::Dict;
    use typst::syntax::{DiagSpan, FileId, RootedPath, Source, VirtualPath};

    use super::*;
    use crate::library::{HostInputs, SiteLibrary};
    use crate::{TOLA_NAMESPACE, TolaPackage};

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

    fn origin(diagnostic: &NativeDiagnostic) -> ReportOrigin {
        ReportOrigin::from_origin(diagnostic.origin()).expect("a typed public report")
    }

    /// Only the in-memory files and builtin sources exist in these worlds.
    struct SiteFiles(FileMap);

    impl FileProvider for SiteFiles {
        fn target(&self, id: FileId) -> Option<FileTarget> {
            if let Some(target) = self.0.target(id) {
                return Some(target);
            }
            let VirtualRoot::Package(package) = id.root() else {
                return None;
            };
            TolaPackage::from_spec(package)
                .and_then(|package| package.file(Path::new(id.vpath().get_without_slash())))
                .map(|contents| FileTarget::Bytes(Arc::from(contents.into_owned().into_bytes())))
        }

        fn owned_namespaces(&self) -> &'static [&'static str] {
            &[TOLA_NAMESPACE, "tola-modules"]
        }
    }

    fn world(files: &[(&str, &str)]) -> TypstWorld {
        let mut map = FileMap::new();
        for (path, text) in files {
            map.insert(file_id(path), text.as_bytes().to_vec());
        }
        let library = SiteLibrary::new(HostInputs {
            site: Dict::new(),
            modules: Dict::new(),
            module_validation: None,
            asset_urls: Dict::new(),
            asset_origins: Dict::new(),
            source_records: Array::new(),
            source_records_by_file: Dict::new(),
            source_origins: Dict::new(),
        });
        let root = std::env::temp_dir().join("tola-public-report");
        TypstWorld::builder(&root.join("main.typ"), &root)
            .with_files(Arc::new(FileResolver::new().with_provider(SiteFiles(map))))
            .with_local_cache()
            .no_fonts()
            .with_shared_library(library.shared())
            .build(&tola_typst::BundleCancellation::default())
            .unwrap()
    }

    fn scan(world: &TypstWorld) -> Diagnostics {
        let scanned = scan_world_observed(world, []);
        scanned
            .result()
            .unwrap_or_else(|error| panic!("report scan failed: {error:?}"))
            .diagnostics()
            .clone()
    }

    #[test]
    fn target_location_keeps_report_owner() {
        let caller = span(
            VirtualRoot::Package("@tola-modules/catalog:0.0.0".parse().unwrap()),
            "helpers/report.typ",
        );
        let target = span(VirtualRoot::Project, "article.typ");
        let native = NativeDiagnostic::from(warning(caller, target, "a-dynamic-rule/中文"));
        let origin = origin(&native);
        assert_eq!(
            origin.caller,
            ReportCaller::Package("@tola-modules/catalog:0.0.0".parse().unwrap())
        );
        assert_eq!(origin.code.as_deref(), Some("a-dynamic-rule/中文"));
        assert_eq!(native.source().span, DiagSpan::from(target));
    }

    #[test]
    fn detached_content_keeps_evidence() {
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
    fn sink_dedups_same_rule_and_source() {
        let first = span(VirtualRoot::Project, "first.typ");
        let second = span(VirtualRoot::Project, "second.typ");
        let package = span(
            VirtualRoot::Package("@tola-modules/other:0.0.0".parse().unwrap()),
            "lib.typ",
        );
        let mut sink = Sink::new();
        sink.warn(warning(first, first, "one"));
        sink.warn(warning(first, first, "one"));
        sink.warn(warning(first, first, "two"));
        sink.warn(warning(first, second, "one"));
        sink.warn(warning(package, first, "one"));
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
    fn related_evidence_survives_sink() {
        let caller = span(VirtualRoot::Project, "main.typ");
        let mut sink = Sink::new();
        for message in ["first context", "second context", "first context"] {
            let notes = SourceDiagnostic::warning(Span::detached(), "same issue")
                .with_spanned_hint(message, Span::detached());
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
        let native = reports
            .into_iter()
            .map(NativeDiagnostic::from)
            .collect::<Vec<_>>();
        assert_eq!(origin(&native[0]), origin(&native[1]));
        assert_ne!(native[0].source().hints, native[1].source().hints);
    }

    #[test]
    fn import_keeps_call_location() {
        let text = format!("{IMPORT}#report(\"a cached issue\", code: \"cached\")");
        let world = world(&[("main.typ", &text)]);
        let cold = scan(&world);
        let warm = scan(&world);
        assert_eq!(cold.raw(), warm.raw());
        assert_eq!(cold.len(), 1);
        let native = &cold.raw()[0];
        assert_eq!(native.source().span.id(), Some(file_id("main.typ")));
        assert_eq!(origin(native).severity, ReportSeverity::Warning);
        assert_eq!(cold.iter().next().unwrap().location.line, Some(2));
    }

    #[test]
    fn targets_keep_native_source_ids() {
        let text = format!(
            "{IMPORT}#import \"authored.typ\": value\n#report(\"review\", at: value, related: ((message: \"original\", at: value),))"
        );
        let diagnostics = scan(&world(&[
            ("main.typ", &text),
            ("authored.typ", "#let value = emph[authored]"),
        ]));
        assert_eq!(diagnostics.len(), 1);
        let native = &diagnostics.raw()[0];
        assert_eq!(native.source().span.id(), Some(file_id("authored.typ")));
        assert_eq!(
            native.source().hints[0].span.id(),
            Some(file_id("authored.typ"))
        );
        assert_eq!(origin(native).caller, ReportCaller::Project);
        assert_eq!(origin(native).hint_count, 0);
    }

    #[test]
    fn accumulated_levels_keep_related_apart() {
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
        let diagnostics = scan(&world(&[("main.typ", &text)]));
        let reports = diagnostics.raw().iter().map(origin).collect::<Vec<_>>();
        assert_eq!(reports.len(), 3, "an error report did not stop evaluation");
        assert_eq!(reports[0].severity, ReportSeverity::Error);
        assert_eq!(reports[1].severity, ReportSeverity::Info);
        assert_eq!(reports[2].severity, ReportSeverity::Hint);
        assert_eq!(reports[0].hint_count, 1);
        let native = diagnostics.raw()[0].source();
        assert_eq!(native.severity, typst::diag::Severity::Warning);
        assert!(native.span.is_detached());
        assert_eq!(native.hints.len(), 2);
        assert!(native.hints[1].span.is_detached());
        assert!(diagnostics.iter().next().unwrap().location.path.is_none());
    }

    #[test]
    fn deferred_reports_survive_realization() {
        let text = format!(
            "{IMPORT}#context {{ for _ in range(2) {{ report(\"realized\", code: \"deferred\", at: none) }} }}"
        );
        let world = world(&[("main.typ", &text)]);
        assert!(
            scan(&world).is_empty(),
            "a context was not realized in the scan"
        );
        let cold = compile_world(&world).unwrap();
        let warm = compile_world(&world).unwrap();
        assert_eq!(cold.diagnostics().raw(), warm.diagnostics().raw());
        let reports = cold
            .diagnostics()
            .raw()
            .iter()
            .filter(|native| ReportOrigin::from_origin(native.origin()).is_some())
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 1, "{:?}", cold.diagnostics().raw());
        assert_eq!(origin(reports[0]).code.as_deref(), Some("deferred"));
        // HTML compilation also emits Typst's own feature warning. It is not a second public
        // report, and any other extra diagnostic must still fail this assertion.
        let preset = tola_typst::diagnostic::DiagnosticFilter::html_export_warning_filter();
        for diagnostic in cold.diagnostics().raw() {
            if ReportOrigin::from_origin(diagnostic.origin()).is_none() {
                assert!(
                    preset.matches(diagnostic),
                    "unexpected diagnostic: {diagnostic:?}"
                );
            }
        }
    }

    #[test]
    fn replacement_bytes_are_preserved() {
        let main = "#include \"first.typ\"\n#include \"second.typ\"";
        let first = format!("{IMPORT}#report(\"same issue\", code: \"same-rule\")");
        let second =
            format!("{IMPORT}// unsaved Unicode: 🦊\n#report(\"same issue\", code: \"same-rule\")");
        let original = scan(&world(&[
            ("main.typ", main),
            ("first.typ", &first),
            ("second.typ", &first),
        ]));
        let replacement = scan(&world(&[
            ("main.typ", main),
            ("first.typ", &first),
            ("second.typ", &second),
        ]));
        assert_eq!(original.len(), 2);
        assert_eq!(replacement.len(), 2);
        let location = replacement
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .location
                    .path
                    .as_deref()
                    .is_some_and(|path| path.ends_with("second.typ"))
            })
            .unwrap();
        assert_eq!(location.location.line, Some(3));
        assert!(
            location
                .location
                .source_lines
                .iter()
                .any(|line| line.text.contains("report"))
        );
    }

    #[test]
    fn arguments_supply_no_owner_or_cause() {
        for arguments in [
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
            let text = format!("{IMPORT}#report(\"issue\", {arguments})");
            let world = world(&[("main.typ", &text)]);
            let result = scan_world_observed(&world, []);
            assert!(result.result().is_err(), "accepted {arguments}");
        }
    }

    #[test]
    fn decoder_rejects_foreign_evidence() {
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
    fn documentation_uses_native_signature() {
        let package = crate::builtin_package(&TolaPackage::Diagnostic.spec()).unwrap();
        assert_eq!(package.exports().collect::<Vec<_>>(), ["report"]);
        let exports = package.export_documentation(&["report".into()]).unwrap();
        let crate::ExportDeclaration::Function(signature) = &exports[0].declaration else {
            panic!("report is a native function");
        };
        assert_eq!(signature.parameters.len(), 6);
        let at = signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == "at")
            .unwrap();
        assert_eq!(at.default.as_deref(), Some("auto"));
        assert!(at.ty_spelling().contains("content"));
    }
}
