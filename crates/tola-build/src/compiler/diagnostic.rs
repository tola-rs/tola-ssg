//! Formatting and collection of Typst diagnostics.

use std::path::Path;
use tola_typst::{DiagnosticFilter, DiagnosticOrigin, Diagnostics};

use crate::filesystem::root_relative;
use tola_diagnostic::{Diagnostic, DiagnosticCause, Severity};

/// Compiler failures as shared diagnostics; presentation belongs to consumers.
pub(crate) fn error_diagnostics(
    error: &anyhow::Error,
    root: &Path,
) -> Option<Vec<tola_diagnostic::Diagnostic>> {
    if let Some(compile) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tola_typst::CompileError>())
    {
        return compile_failure_diagnostics(compile, root);
    }
    // Font failures reach a boundary with no `CompileError` wrapped around them.
    let font = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tola_typst::FontLoadError>())?;
    font_failure(font, root).map(|diagnostic| vec![diagnostic])
}

fn without_export_warnings(diagnostics: &Diagnostics) -> Diagnostics {
    diagnostics.filter_out(&DiagnosticFilter::typst_export_warning_filters())
}

/// One Typst failure as author diagnostics; `None` means cancellation.
pub(super) fn compile_failure_diagnostics(
    compile: &tola_typst::CompileError,
    root: &Path,
) -> Option<Vec<tola_diagnostic::Diagnostic>> {
    if let Some(diagnostics) = compile.diagnostics() {
        let diagnostics = without_export_warnings(diagnostics)
            .iter()
            .map(|diagnostic| {
                resolved_diagnostic(
                    diagnostic.clone(),
                    root,
                    tola_diagnostic::codes::typst::COMPILE,
                )
            })
            .collect::<Vec<_>>();
        return Some(ensure_error_diagnostic(
            diagnostics,
            tola_diagnostic::codes::typst::COMPILE,
            "Tola could not compile the site",
        ));
    }
    if let Some(diagnostics) = compile.raw_diagnostics() {
        let filters = DiagnosticFilter::typst_export_warning_filters();
        let diagnostics = diagnostics
            .iter()
            .filter(|source_diagnostic| {
                !filters
                    .iter()
                    .any(|filter| filter.matches(source_diagnostic))
            })
            .map(|source_diagnostic| {
                if let Some(report) =
                    tola_packages::ReportOrigin::from_origin(source_diagnostic.origin())
                {
                    let reported = source_diagnostic.source();
                    return report_diagnostic(
                        &report,
                        reported.message.to_string(),
                        reported.hints.iter().map(|hint| help(hint.v.to_string())),
                    );
                }
                let source_diagnostic = source_diagnostic.source();
                let mut message = source_diagnostic.message.to_string();
                relativize_message(&mut message, root);
                let mut diagnostic = tola_diagnostic::Diagnostic::new(
                    tola_diagnostic::codes::typst::BUNDLE_EXPORT,
                    typst_severity(source_diagnostic.severity),
                    message,
                );
                for hint in &source_diagnostic.hints {
                    let mut message = hint.v.to_string();
                    relativize_message(&mut message, root);
                    diagnostic = diagnostic.with_help(message);
                }
                diagnostic
            })
            .collect::<Vec<_>>();
        return Some(ensure_error_diagnostic(
            diagnostics,
            tola_diagnostic::codes::typst::BUNDLE_EXPORT,
            "Tola could not export the Bundle",
        ));
    }
    let diagnostic = match compile {
        tola_typst::CompileError::HtmlExport { message } => Diagnostic::new(
            tola_diagnostic::codes::typst::HTML_EXPORT,
            Severity::Error,
            relativized_text(message, root),
        ),
        tola_typst::CompileError::Input { message } => Diagnostic::new(
            tola_diagnostic::codes::typst::INPUT,
            Severity::Error,
            relativized_text(message, root),
        ),
        tola_typst::CompileError::SourceLoad(error) => source_load_diagnostic(error, root)?,
        tola_typst::CompileError::Font(error) => font_failure(error, root)?,
        tola_typst::CompileError::World(error) => world_diagnostic(error, root)?,
        tola_typst::CompileError::Cancelled => return None,
        tola_typst::CompileError::Compilation { .. }
        | tola_typst::CompileError::BundleExport { .. } => unreachable!("handled above"),
    };
    Some(vec![diagnostic])
}

/// One already-written sentence with every site-root prefix rewritten to its site-relative form.
fn relativized_text(text: &str, root: &Path) -> String {
    let mut text = text.to_owned();
    relativize_message(&mut text, root);
    text
}

/// One source load failure as author diagnostics; `None` means cancellation.
fn source_load_diagnostic(
    error: &tola_typst::world::SourceLoadError,
    root: &Path,
) -> Option<Diagnostic> {
    use tola_typst::world::SourceLoadError;
    Some(match error {
        SourceLoadError::Cancelled => return None,
        SourceLoadError::File { path, error } => match site_relative(path, root) {
            Some(relative) => {
                let diagnostic = Diagnostic::new(
                    tola_diagnostic::codes::typst::SNAPSHOT,
                    Severity::Error,
                    format!("Tola could not read `{relative}`"),
                )
                .with_path(relative)
                .with_help("Check that the file exists and is readable, then run the build again");
                with_file_failure_reason(diagnostic, error)
            }
            None => Diagnostic::new(
                tola_diagnostic::codes::typst::SNAPSHOT,
                Severity::Error,
                "Tola could not read a source this build needs",
            )
            .with_help("Check that each content source exists inside the site root"),
        },
        SourceLoadError::OutsideRoot { .. } => Diagnostic::new(
            tola_diagnostic::codes::typst::SNAPSHOT,
            Severity::Error,
            "a source this build reads is outside the site root",
        )
        .with_help("Keep the content root and the Bundle entry inside the site root"),
        SourceLoadError::RootMismatch { .. } => Diagnostic::new(
            tola_diagnostic::codes::typst::SNAPSHOT,
            Severity::Error,
            "Tola could not reuse the sources it read before the site root changed",
        )
        .with_help("Restart `tola dev` to read the sources again"),
    })
}

/// One font failure as author diagnostics; `None` means cancellation.
pub fn font_failure(error: &tola_typst::FontLoadError, root: &Path) -> Option<Diagnostic> {
    use tola_typst::FontLoadError;
    Some(match error {
        FontLoadError::Cancelled => return None,
        FontLoadError::NotDirectory { path } => match site_relative(path, root) {
            Some(relative) => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                format!("`{relative}` is not a directory"),
            )
            .with_path(relative)
            .with_help(
                "`typst.fonts.paths` entries name directories; point this entry at the directory that holds the font file",
            ),
            None => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                "a `typst.fonts.paths` entry is not a directory",
            )
            .with_help("Point each `typst.fonts.paths` entry at a directory that holds font files"),
        },
        FontLoadError::File { path, error } => match site_relative(path, root) {
            Some(relative) => {
                let diagnostic = Diagnostic::new(
                    tola_diagnostic::codes::typst::FONT,
                    Severity::Error,
                    format!("Tola could not read the font file `{relative}`"),
                )
                .with_path(relative)
                .with_help("Check the font file, or remove it from `typst.fonts.paths`");
                with_file_failure_reason(diagnostic, error)
            }
            None => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                "Tola could not read a font file this site configures",
            )
            .with_help("Check the `typst.fonts.paths` directories"),
        },
        FontLoadError::Changed { path } => match site_relative(path, root) {
            Some(relative) => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                format!("the font file `{relative}` changed while Tola was loading fonts"),
            )
            .with_path(relative)
            .with_help("Run the build again without changing font files"),
            None => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                "a font file changed while Tola was loading fonts",
            )
            .with_help("Run the build again without changing font files"),
        },
    })
}

/// One Typst world failure in the site author's words, or `None` for cancellation.
fn world_diagnostic(error: &tola_typst::WorldBuildError, root: &Path) -> Option<Diagnostic> {
    use tola_typst::WorldBuildError;
    Some(match error {
        WorldBuildError::Cancelled => return None,
        WorldBuildError::MainOutsideRoot { .. } => Diagnostic::new(
            tola_diagnostic::codes::typst::WORLD,
            Severity::Error,
            "the Bundle entry is outside the site root",
        )
        .with_help("Set `build.entry` to a file inside the site root"),
        WorldBuildError::MissingFileCache => Diagnostic::new(
            tola_diagnostic::codes::typst::WORLD,
            Severity::Error,
            "the site sources could not be loaded for this build",
        )
        .with_help("Restart `tola dev` and run the build again"),
        WorldBuildError::MissingFonts => Diagnostic::new(
            tola_diagnostic::codes::typst::WORLD,
            Severity::Error,
            "the configured font files could not be loaded for this build",
        )
        .with_help("Restart `tola dev` and run the build again"),
        WorldBuildError::RootMismatch { .. } => Diagnostic::new(
            tola_diagnostic::codes::typst::WORLD,
            Severity::Error,
            "the site root changed while Tola was reading sources",
        )
        .with_help("Restart `tola dev` to read the sources again"),
        WorldBuildError::FixedTimeWithoutDate => Diagnostic::new(
            tola_diagnostic::codes::typst::WORLD,
            Severity::Error,
            "the build date has no calendar day",
        )
        .with_help("Give the build a date that includes its year, month, and day"),
        WorldBuildError::Source(error) => Diagnostic::new(
            tola_diagnostic::codes::typst::WORLD,
            Severity::Error,
            error.to_string(),
        ),
        WorldBuildError::Font(error) => font_failure(error, root)?,
    })
}

/// The author-facing reason for one file failure, or `None` when Typst keeps it internal.
pub(crate) fn file_failure_reason(error: &typst::diag::FileError) -> Option<String> {
    use typst::diag::{FileError, PackageError};
    Some(match error {
        FileError::NotFound(_) => "it does not exist".to_owned(),
        FileError::AccessDenied => "Tola is not allowed to read it".to_owned(),
        FileError::IsDirectory => "it is a directory, not a file".to_owned(),
        FileError::NotSource => "it is not a Typst source file".to_owned(),
        FileError::InvalidUtf8 => "its contents are not valid UTF-8".to_owned(),
        FileError::Realize(_) => "its path is not valid on this platform".to_owned(),
        FileError::Package(PackageError::NotFound(spec)) => {
            format!("the package `{spec}` is not installed")
        }
        FileError::Package(PackageError::VersionNotFound(spec, _)) => {
            format!("the package `{spec}` is not installed")
        }
        FileError::Package(PackageError::NetworkFailed(_)) => {
            "the package could not be downloaded".to_owned()
        }
        FileError::Package(_) => "the package could not be installed".to_owned(),
        FileError::Other(_) => return None,
    })
}

pub(super) fn with_diagnostics(
    error: anyhow::Error,
    root: &Path,
    fallback_code: tola_diagnostic::DiagnosticCode,
) -> anyhow::Error {
    attach_reports(error, |error| {
        error_diagnostics(error, root)
            .unwrap_or_else(|| vec![tola_diagnostic::fallback(fallback_code, error)])
    })
}

/// Attach `reports` for `error`, unless it is cancelled or already carries diagnostics.
pub(crate) fn attach_reports(
    error: anyhow::Error,
    reports: impl FnOnce(&anyhow::Error) -> Vec<Diagnostic>,
) -> anyhow::Error {
    if crate::cancellation::is_cancelled(&error) || tola_diagnostic::attached(&error).is_some() {
        return error;
    }
    let diagnostics = reports(&error);
    anyhow::Error::new(tola_diagnostic::DiagnosticError::attach(error, diagnostics))
}

pub(super) fn with_export_diagnostics(
    error: tola_typst::CompileError,
    world: &tola_typst::TypstWorld,
    compilation: &tola_typst::BundleCompilation,
    root: &Path,
) -> anyhow::Error {
    let Some(raw) = error.raw_diagnostics() else {
        return with_diagnostics(
            error.into(),
            root,
            tola_diagnostic::codes::typst::SITE_PROGRAM,
        );
    };
    let mut diagnostics = Diagnostics::resolve(world, raw);
    diagnostics.attach_imported_by(|package| compilation.files_importing(package).to_vec());
    let diagnostics = without_export_warnings(&diagnostics)
        .into_iter()
        .map(|diagnostic| {
            resolved_diagnostic(
                diagnostic,
                root,
                tola_diagnostic::codes::typst::BUNDLE_EXPORT,
            )
        })
        .collect();
    let diagnostics = ensure_error_diagnostic(
        diagnostics,
        tola_diagnostic::codes::typst::BUNDLE_EXPORT,
        "Tola could not export the Bundle",
    );
    tola_diagnostic::DiagnosticError::attach(error.into(), diagnostics).into()
}

fn ensure_error_diagnostic(
    mut diagnostics: Vec<Diagnostic>,
    code: tola_diagnostic::DiagnosticCode,
    message: &'static str,
) -> Vec<Diagnostic> {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return diagnostics;
    }
    let mut diagnostic = Diagnostic::new(code, Severity::Error, message).with_help(
        "Rerun with `--log-file tola.log`, then report this at \
         https://github.com/tola-rs/tola-ssg/issues",
    );
    if !diagnostics.is_empty() {
        diagnostic = diagnostic.with_note("Typst reported warnings but no error");
    }
    diagnostics.insert(0, diagnostic);
    diagnostics
}

/// The site-root-relative spelling of `path`, when it lies inside the site root.
pub(crate) fn site_relative(path: &Path, root: &Path) -> Option<String> {
    let relative = root_relative(path, root);
    (!relative.as_os_str().is_empty() && relative != path)
        .then(|| crate::filesystem::display_path(path, root))
}

/// Typst's wording when the world provides no clock; a wording change drops the guidance.
const MISSING_CLOCK: &str = "unable to get the current date";

pub(super) fn resolved_diagnostic(
    mut diagnostic: tola_typst::Diagnostic,
    root: &Path,
    code: tola_diagnostic::DiagnosticCode,
) -> tola_diagnostic::Diagnostic {
    if let Some(report) = tola_packages::ReportOrigin::from_origin(&diagnostic.origin) {
        return resolved_report(diagnostic, root, report);
    }
    // Source metadata issues belong to the source; its producer names that ownership.
    let code = match &diagnostic.origin {
        DiagnosticOrigin::Producer(origin)
            if origin.name() == tola_packages::SOURCE_ISSUE_ORIGIN =>
        {
            tola_diagnostic::codes::source::METADATA
        }
        DiagnosticOrigin::Producer(origin)
            if origin.name() == tola_packages::DECLARATION_PROTOCOL_ORIGIN =>
        {
            tola_diagnostic::codes::source::DECLARATION_PROTOCOL
        }
        origin if tola_packages::AssetUrlShadowed::from_origin(origin).is_some() => {
            tola_diagnostic::codes::reference::ASSET_URL_SHADOWED
        }
        _ => code,
    };
    relativize_diagnostic(&mut diagnostic, root);
    let severity = typst_severity(diagnostic.severity);
    let mut notes = Vec::new();
    if let Some(note) = imported_by_note(&diagnostic.imported_by) {
        notes.push(note);
    }
    let mut help = Vec::new();
    if let Some(failure) = &diagnostic.package_failure {
        let guidance = package_failure_guidance(failure, root);
        diagnostic.message = guidance.message;
        notes.extend(guidance.note);
        help.push(guidance.help);
    }
    if diagnostic.message == MISSING_CLOCK {
        diagnostic.message =
            "`datetime.today()` is not available because Tola does not read the current date when building".into();
        help.push(tola_diagnostic::Help {
            message: "Replace `datetime.today()` with an explicit date, such as \
                      `datetime(year: 2026, month: 10, day: 17)`"
                .into(),
            location: None,
        });
    }
    let location = reported_location(&diagnostic.location);
    let cause = unknown_variable_cause(&diagnostic.message, location.as_ref());
    tola_diagnostic::Diagnostic {
        severity,
        code,
        producer: None,
        source: None,
        message: diagnostic.message,
        location,
        imported_by: diagnostic.imported_by,
        notes,
        help: help
            .into_iter()
            .chain(diagnostic.hints.into_iter().map(help_from_hint))
            .collect(),
        related: Vec::new(),
        trace: diagnostic.traces.into_iter().map(trace_frame).collect(),
        cause,
    }
}

fn typst_severity(severity: tola_typst::DiagnosticSeverity) -> Severity {
    match severity {
        tola_typst::DiagnosticSeverity::Error => Severity::Error,
        tola_typst::DiagnosticSeverity::Warning => Severity::Warning,
    }
}

fn report_severity(severity: tola_packages::ReportSeverity) -> Severity {
    match severity {
        tola_packages::ReportSeverity::Error => Severity::Error,
        tola_packages::ReportSeverity::Warning => Severity::Warning,
        tola_packages::ReportSeverity::Info => Severity::Info,
        tola_packages::ReportSeverity::Hint => Severity::Hint,
    }
}

fn report_producer(report: &tola_packages::ReportOrigin) -> tola_diagnostic::DiagnosticProducer {
    tola_diagnostic::DiagnosticProducer {
        name: match &report.caller {
            tola_packages::ReportCaller::Project => Some("project".into()),
            tola_packages::ReportCaller::Package(package) => Some(package.to_string()),
            tola_packages::ReportCaller::Unknown => None,
        },
        code: report.code.clone(),
    }
}

/// One package report: its first `hint_count` hints are help, the rest its reason.
fn report_diagnostic(
    report: &tola_packages::ReportOrigin,
    message: String,
    hints: impl IntoIterator<Item = tola_diagnostic::Help>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        tola_diagnostic::codes::report::ISSUE,
        report_severity(report.severity),
        message,
    );
    diagnostic.producer = Some(report_producer(report));
    for (index, help) in hints.into_iter().enumerate() {
        if index < report.hint_count {
            diagnostic.help.push(help);
        } else {
            diagnostic
                .related
                .push(tola_diagnostic::RelatedInformation {
                    message: help.message,
                    location: help.location,
                    source: None,
                });
        }
    }
    diagnostic
}

fn resolved_report(
    mut diagnostic: tola_typst::Diagnostic,
    root: &Path,
    report: tola_packages::ReportOrigin,
) -> Diagnostic {
    relativize_diagnostic(&mut diagnostic, root);
    let mut result = report_diagnostic(
        &report,
        diagnostic.message,
        diagnostic.hints.into_iter().map(help_from_hint),
    );
    result.location = reported_location(&diagnostic.location);
    result.imported_by = diagnostic.imported_by;
    result.trace = diagnostic.traces.into_iter().map(trace_frame).collect();
    result
}

/// Retained compiler reports in the site diagnostic model, coalesced per producer.
fn resolved_reports(
    config: &crate::config::ResolvedSiteConfig,
    diagnostics: &Diagnostics,
) -> Vec<tola_diagnostic::Diagnostic> {
    let mut resolved = diagnostics
        .iter()
        .map(|diagnostic| {
            resolved_diagnostic(
                diagnostic.clone(),
                config.get_root(),
                tola_diagnostic::codes::typst::COMPILE,
            )
        })
        .collect();
    tola_diagnostic::coalesce_reports(&mut resolved);
    resolved
}

/// Restore report levels before a successful compiler phase can expose its result.
pub(crate) fn ensure_native_reports_succeed(
    config: &crate::config::ResolvedSiteConfig,
    diagnostics: &Diagnostics,
) -> anyhow::Result<()> {
    tola_diagnostic::ensure_reports_succeed(&resolved_reports(config, diagnostics))
}

/// The unbound name a Typst "unknown variable" wording leaves, read from the highlight.
fn unknown_variable_cause(
    message: &str,
    location: Option<&tola_diagnostic::Location>,
) -> Option<DiagnosticCause> {
    const UNKNOWN_VARIABLE: &str = "unknown variable";
    if !message.starts_with(UNKNOWN_VARIABLE) {
        return None;
    }
    let location = location?;
    let name = location
        .source_lines
        .iter()
        .find_map(|line| {
            let (start, end) = line.highlight?;
            if let Some(range) = location.range {
                let start_character =
                    line.start_character + line.text.get(..start)?.encode_utf16().count();
                let end_character =
                    line.start_character + line.text.get(..end)?.encode_utf16().count();
                if range.start.line + 1 != line.line
                    || range.end.line + 1 != line.line
                    || range.start.character != start_character
                    || range.end.character != end_character
                {
                    return None;
                }
            }
            line.text.get(start..end).map(str::trim)
        })
        .filter(|name| !name.is_empty())?;
    Some(DiagnosticCause::UnknownVariable {
        name: name.to_owned(),
    })
}

/// What a site author reads and does about a package Tola could not provide.
struct PackageFailureGuidance {
    message: String,
    note: Option<String>,
    help: tola_diagnostic::Help,
}

/// The declared package directory leads; `--package-path` reaches one kept elsewhere.
fn package_failure_guidance(
    failure: &tola_typst::ResolvedPackageFailure,
    root: &Path,
) -> PackageFailureGuidance {
    use tola_typst::ResolvedPackageFailureReason;

    let package = &failure.package;
    let site_search = failure
        .searched
        .iter()
        .find_map(|search| Some((site_relative(&search.root, root)?, search)));
    let directory = site_search
        .as_ref()
        .map(|(directory, _)| directory.as_str())
        .filter(|directory| !directory.is_empty());
    match failure.reason {
        ResolvedPackageFailureReason::NotInstalled => match owned_namespace_guidance(package) {
            Some(guidance) => guidance,
            None => PackageFailureGuidance {
                message: format!("the package `{package}` is not installed"),
                note: site_search.as_ref().and_then(|(_, search)| {
                    site_relative(&search.candidate, root)
                        .map(|candidate| format!("`{candidate}` does not exist"))
                }),
                help: help(match directory {
                    Some(directory) => format!("Put the package in `{directory}/`"),
                    None => "Run `tola vendor` to add the package to the site".to_owned(),
                }),
            },
        },
        ResolvedPackageFailureReason::Unavailable => PackageFailureGuidance {
            message: format!("Tola could not install the package `{package}`"),
            note: None,
            help: help(match directory {
                Some(directory) => {
                    format!("Add it to `{directory}/` so the site includes the package itself")
                }
                None => "Make `.tola` writable".to_owned(),
            }),
        },
    }
}

fn help(message: impl Into<String>) -> tola_diagnostic::Help {
    tola_diagnostic::Help {
        message: message.into(),
        location: None,
    }
}

fn owned_namespace_guidance(package: &str) -> Option<PackageFailureGuidance> {
    let spec = package
        .parse::<typst::syntax::package::PackageSpec>()
        .ok()?;
    if spec.namespace.as_str() != tola_packages::TOLA_NAMESPACE {
        return None;
    }
    let provided = tola_packages::TolaPackage::all()
        .iter()
        .copied()
        .find(|candidate| candidate.name() == spec.name.as_str());
    Some(match provided {
        Some(provided) => PackageFailureGuidance {
            message: format!("Tola provides `{}`, not `{package}`", provided.spec()),
            note: None,
            help: help(format!("Import `{}`", provided.spec())),
        },
        None => PackageFailureGuidance {
            message: format!("`{package}` is not a Tola package"),
            note: None,
            help: help("Check the package name"),
        },
    })
}

fn source_line(source: tola_typst::SourceLine) -> tola_diagnostic::SourceLine {
    let highlight = source
        .highlight
        .and_then(|range| char_range_to_byte_range(&source.text, range));
    tola_diagnostic::SourceLine {
        start_column: source.start_column,
        start_character: source.start_character,
        ends_line: source.ends_line,
        ..tola_diagnostic::SourceLine::new(source.line_num, source.text, highlight)
    }
}

fn char_range_to_byte_range(text: &str, (start, end): (usize, usize)) -> Option<(usize, usize)> {
    if start >= end {
        return None;
    }
    let byte_offset = |column| {
        text.char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(text.len()))
            .nth(column)
    };
    Some((byte_offset(start)?, byte_offset(end)?))
}

fn with_file_failure_reason(diagnostic: Diagnostic, error: &typst::diag::FileError) -> Diagnostic {
    match file_failure_reason(error) {
        Some(reason) => diagnostic.with_note(reason),
        None => diagnostic,
    }
}

fn diagnostic_location(source: &tola_typst::SourceLocation) -> tola_diagnostic::Location {
    tola_diagnostic::Location {
        path: source.path.clone().unwrap_or_default(),
        line: source.line,
        column: source.column,
        range: source.range.map(|range| tola_diagnostic::SourceRange {
            start: tola_diagnostic::SourcePosition {
                line: range.start.line,
                character: range.start.character,
            },
            end: tola_diagnostic::SourcePosition {
                line: range.end.line,
                character: range.end.character,
            },
        }),
        source_lines: source
            .source_lines
            .iter()
            .cloned()
            .map(source_line)
            .collect(),
    }
}

/// Locate generated content against the immutable source used by its compilation.
pub(crate) fn source_location(
    world: &tola_typst::TypstWorld,
    span: typst::syntax::Span,
) -> Option<tola_diagnostic::Location> {
    tola_typst::diagnostic::resolve_source_location(world, span)
        .map(|source| diagnostic_location(&source))
}

fn reported_location(location: &tola_typst::SourceLocation) -> Option<tola_diagnostic::Location> {
    location
        .path
        .as_ref()
        .map(|_| diagnostic_location(location))
}

fn help_from_hint(hint: tola_typst::Hint) -> tola_diagnostic::Help {
    tola_diagnostic::Help {
        message: hint.message,
        location: reported_location(&hint.location),
    }
}

fn trace_frame(trace: tola_typst::Trace) -> tola_diagnostic::TraceFrame {
    tola_diagnostic::TraceFrame {
        message: trace.message,
        location: reported_location(&trace.location),
    }
}

/// Static source navigation, not a claim that each import or include executed.
fn imported_by_note(files: &[String]) -> Option<String> {
    (!files.is_empty()).then(|| {
        format!(
            "package references in {}",
            tola_diagnostic::bounded_listing(files, ("file", "files"))
        )
    })
}

fn relativize_diagnostic(diagnostic: &mut tola_typst::Diagnostic, root: &Path) {
    fn relativize(location: &mut tola_typst::SourceLocation, root: &Path) {
        if let Some(path) = &location.path {
            location.path = Some(crate::filesystem::display_path(Path::new(path), root));
        }
    }

    relativize_message(&mut diagnostic.message, root);
    for hint in &mut diagnostic.hints {
        relativize_message(&mut hint.message, root);
        relativize(&mut hint.location, root);
    }
    for trace in &mut diagnostic.traces {
        relativize_message(&mut trace.message, root);
        relativize(&mut trace.location, root);
    }
    relativize(&mut diagnostic.location, root);
}

/// Rewrite every site-root prefix inside `text`; Typst names files absolutely.
fn relativize_message(text: &mut String, root: &Path) {
    let root = root.to_string_lossy();
    let root = root.trim_end_matches(['/', '\\']);
    if !root.is_empty() {
        *text = replace_root_prefix(text, root);
    }
}

fn replace_root_prefix(text: &str, root: &str) -> String {
    let mut rewritten = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(root) {
        let (before, matched) = rest.split_at(index);
        let after = &matched[root.len()..];
        let Some(relative) = site_relative_after_root(before, after) else {
            rewritten.push_str(&rest[..index + root.len()]);
            rest = after;
            continue;
        };
        rewritten.push_str(before);
        for character in relative.chars() {
            rewritten.push(if character == '\\' { '/' } else { character });
        }
        rest = &after[1 + relative.len()..];
    }
    rewritten.push_str(rest);
    rewritten
}

/// The path a matched site root introduces, when the match names the root itself.
fn site_relative_after_root<'a>(before: &str, after: &'a str) -> Option<&'a str> {
    let continues_component = before.chars().next_back().is_some_and(|previous| {
        previous.is_alphanumeric() || matches!(previous, '/' | '\\' | '_' | '-' | '.')
    }) || after
        .chars()
        .next()
        .is_some_and(|next| next.is_alphanumeric() || matches!(next, '.' | '-' | '_'));
    if continues_component {
        return None;
    }
    let relative = relative_path_run(after.strip_prefix(['/', '\\'])?);
    (!relative.is_empty()).then_some(relative)
}

/// The path run at the start of `path`, ending before whitespace or the punctuation around it.
fn relative_path_run(path: &str) -> &str {
    let end = path
        .find(|character: char| {
            character.is_whitespace()
                || matches!(character, '(' | ')' | '`' | '\'' | '"' | ',' | ';')
        })
        .unwrap_or(path.len());
    &path[..end]
}

/// Resolve the complete retained warning set. Display limits belong to consumers.
pub(crate) fn warning_diagnostics(
    config: &crate::config::ResolvedSiteConfig,
    warnings: &mut Diagnostics,
) -> Vec<tola_diagnostic::Diagnostic> {
    let retained = std::mem::take(warnings);
    let warnings = without_export_warnings(&retained);
    resolved_reports(config, &warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A location that names only the fields a case sets.
    fn location(path: Option<&str>) -> tola_typst::SourceLocation {
        tola_typst::SourceLocation {
            path: path.map(str::to_owned),
            line: None,
            column: None,
            range: None,
            source_lines: Vec::new(),
            location_failure: None,
            truncation: tola_typst::SourceTruncation::default(),
        }
    }

    /// A warning at `location`, with no hints or traces.
    fn warning_at(message: &str, location: tola_typst::SourceLocation) -> tola_typst::Diagnostic {
        tola_typst::Diagnostic {
            origin: tola_typst::DiagnosticOrigin::Typst,
            severity: tola_typst::DiagnosticSeverity::Warning,
            message: message.into(),
            location,
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        }
    }

    /// Every failure kind maps to its own stable code and never names a path outside the site.
    ///
    /// The code is the contract a consumer reads; the sentence is presentation and is free to
    /// change, so nothing here asserts one.
    #[test]
    fn failures_carry_their_own_code() {
        let site = Path::new("/site");
        let outside = anyhow::Error::new(tola_typst::CompileError::World(
            tola_typst::WorldBuildError::MainOutsideRoot {
                main: "/other/site.typ".into(),
                root: "/site".into(),
            },
        ));
        let source_load = anyhow::Error::new(tola_typst::CompileError::SourceLoad(
            tola_typst::world::SourceLoadError::File {
                path: "/site/content/post.typ".into(),
                error: typst::diag::FileError::AccessDenied,
            },
        ));
        let font = anyhow::Error::new(tola_typst::CompileError::Font(
            tola_typst::FontLoadError::File {
                path: "/site/static/typst-fonts/Inter.ttf".into(),
                error: typst::diag::FileError::NotFound(
                    "/site/static/typst-fonts/Inter.ttf".into(),
                ),
            },
        ));
        let cancelled = anyhow::Error::new(tola_typst::CompileError::Cancelled);
        let export = anyhow::Error::new(tola_typst::CompileError::bundle_export([
            typst::diag::SourceDiagnostic::error(typst::syntax::Span::detached(), "broken").into(),
        ]));

        for (label, error, code) in [
            ("world", &outside, tola_diagnostic::codes::typst::WORLD),
            (
                "source load",
                &source_load,
                tola_diagnostic::codes::typst::SNAPSHOT,
            ),
            ("font", &font, tola_diagnostic::codes::typst::FONT),
            (
                "export",
                &export,
                tola_diagnostic::codes::typst::BUNDLE_EXPORT,
            ),
        ] {
            let diagnostics = error_diagnostics(error, site)
                .unwrap_or_else(|| panic!("{label} produced no diagnostic"));
            assert_eq!(diagnostics[0].code, code, "{label}");
            for diagnostic in &diagnostics {
                assert!(
                    !diagnostic.message.contains("/other/")
                        && !diagnostic.message.contains("/site/"),
                    "{label} leaked a host path: {}",
                    diagnostic.message
                );
            }
        }

        // Cancellation is reported as cancellation, never as a failure to describe.
        assert!(error_diagnostics(&cancelled, site).is_none());
    }

    /// A located failure reports the site-relative path, and an outside path is dropped.
    #[test]
    fn failure_locations_are_site_relative() {
        let source_load = |path: &str| {
            anyhow::Error::new(tola_typst::CompileError::SourceLoad(
                tola_typst::world::SourceLoadError::File {
                    path: path.into(),
                    error: typst::diag::FileError::AccessDenied,
                },
            ))
        };

        let inside = error_diagnostics(&source_load("/site/content/post.typ"), Path::new("/site"))
            .expect("an inside path is described");
        assert_eq!(
            inside[0].location.as_ref().map(|found| found.path.as_str()),
            Some("content/post.typ")
        );

        let outside = error_diagnostics(&source_load("/elsewhere/post.typ"), Path::new("/site"))
            .expect("an outside path is still described");
        assert!(outside[0].location.is_none(), "{outside:#?}");
    }

    /// A package failure names the site directory the author can act on, never the host cache.
    #[test]
    fn package_failure_names_directory() {
        use tola_typst::{
            ResolvedPackageFailure, ResolvedPackageFailureReason, ResolvedPackageSearch,
        };

        let root = Path::new("/site");
        let site_packages = |package: &str, version: &str| ResolvedPackageSearch {
            root: root.join("packages"),
            candidate: root.join("packages").join(package).join(version),
        };
        let host_cache = |package: &str, version: &str| ResolvedPackageSearch {
            root: PathBuf::from("/home/author/.cache/typst/packages"),
            candidate: PathBuf::from("/home/author/.cache/typst/packages")
                .join(package)
                .join(version),
        };

        for (package, reason, searched, names_site) in [
            (
                "@cawemo/report:1.2.0",
                ResolvedPackageFailureReason::NotInstalled,
                vec![
                    site_packages("cawemo/report", "1.2.0"),
                    host_cache("cawemo/report", "1.2.0"),
                ],
                true,
            ),
            (
                "@preview/cetz:0.3.4",
                ResolvedPackageFailureReason::Unavailable,
                vec![
                    site_packages("preview/cetz", "0.3.4"),
                    host_cache("preview/cetz", "0.3.4"),
                ],
                true,
            ),
            (
                "@preview/cetz:0.3.4",
                ResolvedPackageFailureReason::NotInstalled,
                vec![host_cache("preview/cetz", "0.3.4")],
                false,
            ),
        ] {
            let failure = ResolvedPackageFailure {
                package: package.into(),
                reason,
                searched,
            };
            let guidance = package_failure_guidance(&failure, root);

            assert!(guidance.message.contains(package), "{package} {reason:?}");
            let help = &guidance.help;
            assert!(!help.message.contains("author"), "{}", help.message);
            assert!(!help.message.contains("/home/"), "{}", help.message);
            assert_eq!(
                help.message.contains("`packages/`"),
                names_site,
                "{package} {reason:?}: {}",
                help.message
            );
        }
    }

    /// Display limits never drop a warning, and a warning-only failure still reports an error.
    #[test]
    fn failure_keeps_its_warnings() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let make_warning = |message: &str| warning_at(message, location(None));
        let mut warnings = Diagnostics::new();
        warnings.extend_distinct(&Diagnostics::from_vec(vec![
            make_warning("first warning"),
            make_warning("second warning"),
        ]));
        assert_eq!(warning_diagnostics(&config, &mut warnings).len(), 2);

        let compiler_warning = warning_at("upstream warning", location(None));
        let export_warning = typst::diag::SourceDiagnostic::warning(
            typst::syntax::Span::detached(),
            "upstream warning",
        );
        for error in [
            anyhow::Error::new(tola_typst::CompileError::Compilation {
                diagnostics: Diagnostics::from_vec(vec![compiler_warning]),
            }),
            anyhow::Error::new(tola_typst::CompileError::bundle_export([
                export_warning.into()
            ])),
        ] {
            let diagnostics = error_diagnostics(&error, Path::new("/site")).unwrap();
            assert_eq!(diagnostics.len(), 2);
            assert_eq!(diagnostics[0].severity, tola_diagnostic::Severity::Error);
            assert_eq!(diagnostics[1].severity, tola_diagnostic::Severity::Warning);
        }
    }

    /// A site-root prefix is stripped from a message, and a mere textual sibling is not.
    #[test]
    fn site_root_prefixes_become_relative() {
        let mut message = String::from("cannot read /site/content/post.typ");
        relativize_message(&mut message, Path::new("/site"));
        assert_eq!(message, "cannot read content/post.typ");

        for untouched in [
            "cannot read /other/site/content/post.typ",
            "cannot read /site-sibling/post.typ",
        ] {
            let mut message = untouched.to_owned();
            relativize_message(&mut message, Path::new("/site"));
            assert_eq!(message, untouched);
        }
    }

    /// A highlight's character offsets become byte offsets, and a path keeps its site-relative
    /// shape at every depth of one diagnostic.
    #[test]
    fn converted_paths_stay_relative() {
        let line = source_line(tola_typst::SourceLine {
            line_num: 1,
            start_column: 0,
            start_character: 0,
            ends_line: true,
            text: "éx".into(),
            highlight: Some((1, 2)),
        });
        assert_eq!(line.highlight, Some((2, 3)));

        let resolved = tola_typst::Diagnostic {
            hints: vec![tola_typst::Hint {
                message: "inspect the import".into(),
                location: tola_typst::SourceLocation {
                    line: Some(1),
                    column: Some(1),
                    ..location(Some("/site/templates/document.typ"))
                },
            }],
            traces: vec![tola_typst::Trace {
                kind: tola_typst::TraceKind::Include("content/index.typ".into()),
                message: "while including `content/index.typ`".into(),
                location: location(Some("/site/site.typ")),
            }],
            ..warning_at(
                "warning",
                tola_typst::SourceLocation {
                    line: Some(2),
                    column: Some(3),
                    ..location(Some("/site/content/index.typ"))
                },
            )
        };
        let diagnostic = resolved_diagnostic(
            resolved,
            Path::new("/site"),
            tola_diagnostic::codes::typst::COMPILE,
        );

        assert_eq!(diagnostic.code, tola_diagnostic::codes::typst::COMPILE);
        assert_eq!(diagnostic.location.unwrap().path, "content/index.typ");
        assert_eq!(
            diagnostic.help[0].location.as_ref().unwrap().path,
            "templates/document.typ"
        );
        assert_eq!(
            diagnostic.trace[0].location.as_ref().unwrap().path,
            "site.typ"
        );
    }

    /// An unbound name is read from the highlight, so the cause survives a wording change and
    /// appears only when a highlight actually names one.
    #[test]
    fn unknown_variable_follows_highlight() {
        let highlighted = tola_typst::Diagnostic {
            origin: tola_typst::DiagnosticOrigin::Typst,
            severity: tola_typst::DiagnosticSeverity::Error,
            message: "unknown variable: absent-name".into(),
            location: tola_typst::SourceLocation {
                line: Some(1),
                column: Some(15),
                source_lines: vec![tola_typst::SourceLine {
                    line_num: 1,
                    start_column: 0,
                    start_character: 0,
                    ends_line: true,
                    text: "#let missing = absent-name".into(),
                    highlight: Some((15, 26)),
                }],
                ..location(Some("/site/content/index.typ"))
            },
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        };
        let resolved = resolved_diagnostic(
            highlighted,
            Path::new("/site"),
            tola_diagnostic::codes::typst::COMPILE,
        );
        assert_eq!(
            resolved.cause,
            Some(tola_diagnostic::DiagnosticCause::UnknownVariable {
                name: "absent-name".to_owned(),
            })
        );

        // No highlight, another failure, and a panic that quotes the sentence all carry no cause.
        let without_highlight = tola_typst::Diagnostic {
            location: location(Some("/site/content/index.typ")),
            ..warning_at("unknown variable: absent-name", location(None))
        };
        let panic = tola_typst::Diagnostic {
            message: "panicked with: unknown variable: absent-name".into(),
            ..without_highlight.clone()
        };
        for diagnostic in [without_highlight, panic] {
            let resolved = resolved_diagnostic(
                diagnostic,
                Path::new("/site"),
                tola_diagnostic::codes::typst::COMPILE,
            );
            assert_eq!(resolved.cause, None, "{resolved:?}");
        }
    }

    /// A missing clock names the repair at the site-author level: the cause is the code, and the
    /// help must mention `datetime.today()`.
    #[test]
    fn missing_clock_help_names_datetime() {
        let diagnostic = tola_typst::Diagnostic {
            origin: tola_typst::DiagnosticOrigin::Typst,
            severity: tola_typst::DiagnosticSeverity::Error,
            message: MISSING_CLOCK.into(),
            location: location(Some("/site/site.typ")),
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        };

        let resolved = resolved_diagnostic(
            diagnostic,
            Path::new("/site"),
            tola_diagnostic::codes::typst::COMPILE,
        );

        assert!(resolved.message.contains("datetime.today()"));
        assert!(
            resolved
                .help
                .iter()
                .any(|help| help.message.contains("datetime(year:")),
            "{resolved:#?}"
        );
    }
}
