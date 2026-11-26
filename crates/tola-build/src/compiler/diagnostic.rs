//! Formatting and collection of Typst diagnostics.

mod location;
#[cfg(test)]
mod tests;

use std::path::Path;

use tola_diagnostic::{Diagnostic, DiagnosticCause, Severity};
use tola_typst::{DiagnosticFilter, DiagnosticOrigin, Diagnostics};

use location::{
    help_from_hint, relativize_message, relativize_messages, relativized_text, reported_location,
    trace_frame,
};
pub(crate) use location::{site_relative, source_location};

const MISSING_CLOCK: &str = "unable to get the current date";

/// Compiler failures as shared diagnostics; presentation belongs to consumers.
pub(crate) fn error_diagnostics(error: &anyhow::Error, root: &Path) -> Option<Vec<Diagnostic>> {
    if let Some(compile) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tola_typst::CompileError>())
    {
        return compile_failure_diagnostics(compile, root);
    }
    let font = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tola_typst::FontLoadError>())?;
    font_failure(font, root).map(|diag| vec![diag])
}

fn without_export_warnings(diags: &Diagnostics) -> Diagnostics {
    diags.filter_out(&DiagnosticFilter::typst_export_warning_filters())
}

/// One Typst failure as author diagnostics; `None` means cancellation.
pub(super) fn compile_failure_diagnostics(
    compile: &tola_typst::CompileError,
    root: &Path,
) -> Option<Vec<Diagnostic>> {
    if let Some(diags) = compile.diagnostics() {
        let diags = without_export_warnings(diags)
            .iter()
            .map(|d| convert_diagnostic(d.clone(), root, tola_diagnostic::codes::typst::COMPILE))
            .collect();
        return Some(ensure_error_diagnostic(
            diags,
            tola_diagnostic::codes::typst::COMPILE,
            "Tola could not compile the site",
        ));
    }
    if let Some(diags) = compile.raw_diagnostics() {
        let filters = DiagnosticFilter::typst_export_warning_filters();
        let diags = diags
            .iter()
            .filter(|sd| !filters.iter().any(|f| f.matches(sd)))
            .map(|sd| {
                if let Some(report) = tola_packages::ReportOrigin::from_origin(sd.origin()) {
                    let reported = sd.source();
                    return report_diagnostic(
                        &report,
                        reported.message.to_string(),
                        reported.hints.iter().map(|h| help(h.v.to_string())),
                    );
                }
                let sd = sd.source();
                let mut msg = sd.message.to_string();
                relativize_message(&mut msg, root);
                let mut diag = Diagnostic::new(
                    tola_diagnostic::codes::typst::BUNDLE_EXPORT,
                    typst_severity(sd.severity),
                    msg,
                );
                for hint in &sd.hints {
                    let mut msg = hint.v.to_string();
                    relativize_message(&mut msg, root);
                    diag = diag.with_help(msg);
                }
                diag
            })
            .collect();
        return Some(ensure_error_diagnostic(
            diags,
            tola_diagnostic::codes::typst::BUNDLE_EXPORT,
            "Tola could not export the Bundle",
        ));
    }
    let diag = match compile {
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
        tola_typst::CompileError::SourceLoad(err) => source_load_diagnostic(err, root)?,
        tola_typst::CompileError::Font(err) => font_failure(err, root)?,
        tola_typst::CompileError::World(err) => world_diagnostic(err, root)?,
        tola_typst::CompileError::Cancelled => return None,
        tola_typst::CompileError::Compilation { .. }
        | tola_typst::CompileError::BundleExport { .. } => unreachable!("handled above"),
    };
    Some(vec![diag])
}

fn source_load_diagnostic(
    err: &tola_typst::world::SourceLoadError,
    root: &Path,
) -> Option<Diagnostic> {
    use tola_typst::world::SourceLoadError;
    Some(match err {
        SourceLoadError::Cancelled => return None,
        SourceLoadError::File { path, error } => match site_relative(path, root) {
            Some(rel) => {
                let diag = Diagnostic::new(
                    tola_diagnostic::codes::typst::SNAPSHOT,
                    Severity::Error,
                    format!("Tola could not read `{rel}`"),
                )
                .with_path(rel)
                .with_help("Check that the file exists and is readable, then run the build again");
                with_file_failure_reason(diag, error)
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
            Some(rel) => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                format!("`{rel}` is not a directory"),
            )
            .with_path(rel)
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
            Some(rel) => {
                let diag = Diagnostic::new(
                    tola_diagnostic::codes::typst::FONT,
                    Severity::Error,
                    format!("Tola could not read the font file `{rel}`"),
                )
                .with_path(rel)
                .with_help("Check the font file, or remove it from `typst.fonts.paths`");
                with_file_failure_reason(diag, error)
            }
            None => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                "Tola could not read a font file this site configures",
            )
            .with_help("Check the `typst.fonts.paths` directories"),
        },
        FontLoadError::Changed { path } => match site_relative(path, root) {
            Some(rel) => Diagnostic::new(
                tola_diagnostic::codes::typst::FONT,
                Severity::Error,
                format!("the font file `{rel}` changed while Tola was loading fonts"),
            )
            .with_path(rel)
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

fn world_diagnostic(err: &tola_typst::WorldBuildError, root: &Path) -> Option<Diagnostic> {
    use tola_typst::WorldBuildError;
    Some(match err {
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
        WorldBuildError::Source(err) => Diagnostic::new(
            tola_diagnostic::codes::typst::WORLD,
            Severity::Error,
            err.to_string(),
        ),
        WorldBuildError::Font(err) => font_failure(err, root)?,
    })
}

pub(crate) fn file_failure_reason(error: &typst::diag::FileError) -> Option<String> {
    use typst::diag::{FileError, PackageError};
    Some(match error {
        FileError::NotFound(_) => "it does not exist".to_owned(),
        FileError::AccessDenied => "Tola is not allowed to read it".to_owned(),
        FileError::IsDirectory => "it is a directory, not a file".to_owned(),
        FileError::NotSource => "it is not a Typst source file".to_owned(),
        FileError::InvalidUtf8 => "its contents are not valid UTF-8".to_owned(),
        FileError::Realize(_) => "its path is not valid on this platform".to_owned(),
        FileError::Package(
            PackageError::NotFound(spec) | PackageError::VersionNotFound(spec, _),
        ) => {
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
    err: anyhow::Error,
    root: &Path,
    fallback_code: tola_diagnostic::DiagnosticCode,
) -> anyhow::Error {
    attach_reports(err, |err| {
        error_diagnostics(err, root)
            .unwrap_or_else(|| vec![tola_diagnostic::fallback(fallback_code, err)])
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
    let diags = reports(&error);
    anyhow::Error::new(tola_diagnostic::DiagnosticError::attach(error, diags))
}

pub(super) fn with_export_diagnostics(
    err: tola_typst::CompileError,
    world: &tola_typst::TypstWorld,
    compilation: &tola_typst::BundleCompilation,
    root: &Path,
) -> anyhow::Error {
    let Some(raw) = err.raw_diagnostics() else {
        return with_diagnostics(
            err.into(),
            root,
            tola_diagnostic::codes::typst::SITE_PROGRAM,
        );
    };
    let mut diags = Diagnostics::resolve(world, raw);
    diags.attach_imported_by(|pkg| compilation.files_importing(pkg).to_vec());
    let diags = without_export_warnings(&diags)
        .into_iter()
        .map(|d| convert_diagnostic(d, root, tola_diagnostic::codes::typst::BUNDLE_EXPORT))
        .collect();
    let diags = ensure_error_diagnostic(
        diags,
        tola_diagnostic::codes::typst::BUNDLE_EXPORT,
        "Tola could not export the Bundle",
    );
    tola_diagnostic::DiagnosticError::attach(err.into(), diags).into()
}

fn ensure_error_diagnostic(
    mut diags: Vec<Diagnostic>,
    code: tola_diagnostic::DiagnosticCode,
    msg: &'static str,
) -> Vec<Diagnostic> {
    if diags.iter().any(|d| d.severity == Severity::Error) {
        return diags;
    }
    let mut diag = Diagnostic::new(code, Severity::Error, msg).with_help(
        "Rerun with `--log-file tola.log`, then report this at \
         https://github.com/tola-rs/tola-ssg/issues",
    );
    if !diags.is_empty() {
        diag = diag.with_note("Typst reported warnings but no error");
    }
    diags.insert(0, diag);
    diags
}

pub(super) fn convert_diagnostic(
    mut diag: tola_typst::Diagnostic,
    root: &Path,
    code: tola_diagnostic::DiagnosticCode,
) -> Diagnostic {
    if let Some(report) = tola_packages::ReportOrigin::from_origin(&diag.origin) {
        return convert_report(diag, root, report);
    }
    let code = match &diag.origin {
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
    relativize_messages(&mut diag, root);
    let severity = typst_severity(diag.severity);
    let mut notes = Vec::new();
    if let Some(note) = imported_by_note(&diag.imported_by) {
        notes.push(note);
    }
    let mut help = Vec::new();
    if let Some(failure) = &diag.package_failure {
        let guidance = package_failure_guidance(failure, root);
        diag.message = guidance.message;
        notes.extend(guidance.note);
        help.push(guidance.help);
    }
    if diag.message == MISSING_CLOCK {
        diag.message =
            "`datetime.today()` is not available because Tola does not read the current date when building".into();
        help.push(tola_diagnostic::Help {
            message: "Replace `datetime.today()` with an explicit date, such as \
                      `datetime(year: 2026, month: 10, day: 17)`"
                .into(),
            location: None,
        });
    }
    let loc = reported_location(&diag.location, root);
    let cause = unknown_variable_cause(&diag.message, loc.as_ref());
    if let Some(DiagnosticCause::UnknownVariable { name }) = &cause {
        diag.message = format!("the name `{name}` is not defined here");
        if diag.hints.is_empty() {
            help.push(tola_diagnostic::Help {
                message: "Check the spelling or define the name before using it".into(),
                location: None,
            });
        }
    }
    Diagnostic {
        severity,
        code,
        producer: None,
        source: None,
        message: diag.message,
        location: loc,
        imported_by: diag.imported_by,
        notes,
        help: help
            .into_iter()
            .chain(diag.hints.into_iter().map(|h| help_from_hint(h, root)))
            .collect(),
        related: Vec::new(),
        trace: diag
            .traces
            .into_iter()
            .map(|t| trace_frame(t, root))
            .collect(),
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
            tola_packages::ReportCaller::Package(pkg) => Some(pkg.to_string()),
            tola_packages::ReportCaller::Unknown => None,
        },
        code: report.code.clone(),
    }
}

fn report_diagnostic(
    report: &tola_packages::ReportOrigin,
    msg: String,
    hints: impl IntoIterator<Item = tola_diagnostic::Help>,
) -> Diagnostic {
    let mut diag = Diagnostic::new(
        tola_diagnostic::codes::report::ISSUE,
        report_severity(report.severity),
        msg,
    );
    diag.producer = Some(report_producer(report));
    for (idx, h) in hints.into_iter().enumerate() {
        if idx < report.hint_count {
            diag.help.push(h);
        } else {
            diag.related.push(tola_diagnostic::RelatedInformation {
                message: h.message,
                location: h.location,
                source: None,
            });
        }
    }
    diag
}

fn convert_report(
    mut diag: tola_typst::Diagnostic,
    root: &Path,
    report: tola_packages::ReportOrigin,
) -> Diagnostic {
    relativize_messages(&mut diag, root);
    let mut res = report_diagnostic(
        &report,
        diag.message,
        diag.hints.into_iter().map(|h| help_from_hint(h, root)),
    );
    res.location = reported_location(&diag.location, root).map(|mut at| {
        at.file = at.editor_file(root);
        at
    });
    res.imported_by = diag.imported_by;
    res.trace = diag
        .traces
        .into_iter()
        .map(|t| trace_frame(t, root))
        .collect();
    res
}

fn convert_reports(root: &Path, diags: &Diagnostics) -> Vec<Diagnostic> {
    let mut resolved: Vec<_> = diags
        .iter()
        .map(|d| convert_diagnostic(d.clone(), root, tola_diagnostic::codes::typst::COMPILE))
        .collect();
    tola_diagnostic::coalesce_reports(&mut resolved);
    resolved
}

pub(crate) fn ensure_native_reports_succeed(
    root: &Path,
    diags: &Diagnostics,
) -> Result<(), tola_diagnostic::DiagnosticError> {
    tola_diagnostic::ensure_reports_succeed(&convert_reports(root, diags))
}

fn unknown_variable_cause(
    msg: &str,
    loc: Option<&tola_diagnostic::Location>,
) -> Option<DiagnosticCause> {
    const UNKNOWN_VARIABLE: &str = "unknown variable";
    if !msg.starts_with(UNKNOWN_VARIABLE) {
        return None;
    }
    let loc = loc?;
    let name = loc
        .source_lines
        .iter()
        .find_map(|line| {
            let (start, end) = line.highlight?;
            if let Some(range) = loc.range {
                let start_ch =
                    line.start_character + line.text.get(..start)?.encode_utf16().count();
                let end_ch = line.start_character + line.text.get(..end)?.encode_utf16().count();
                if range.start.line + 1 != line.line
                    || range.end.line + 1 != line.line
                    || range.start.character != start_ch
                    || range.end.character != end_ch
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

struct PackageFailureGuidance {
    message: String,
    note: Option<String>,
    help: tola_diagnostic::Help,
}

fn package_failure_guidance(
    failure: &tola_typst::PackageFailure,
    root: &Path,
) -> PackageFailureGuidance {
    use tola_typst::PackageFailureReason;

    let pkg = &failure.package;
    let site_search = failure
        .searched
        .iter()
        .find_map(|s| Some((site_relative(&s.root, root)?, s)));
    let dir = site_search
        .as_ref()
        .map(|(d, _)| d.as_str())
        .filter(|d| !d.is_empty());
    match failure.reason {
        PackageFailureReason::NotInstalled => match owned_namespace_guidance(pkg) {
            Some(guidance) => guidance,
            None => PackageFailureGuidance {
                message: format!("the package `{pkg}` is not installed"),
                note: site_search.as_ref().and_then(|(_, s)| {
                    site_relative(&s.candidate, root).map(|cand| format!("`{cand}` does not exist"))
                }),
                help: help(match dir {
                    Some(dir) => format!("Put the package in `{dir}/`"),
                    None => "Run `tola vendor` to add the package to the site".to_owned(),
                }),
            },
        },
        PackageFailureReason::Unavailable => PackageFailureGuidance {
            message: format!("Tola could not install the package `{pkg}`"),
            note: None,
            help: help(match dir {
                Some(dir) => format!("Add it to `{dir}/` so the site includes the package itself"),
                None => "Make `.tola` writable".to_owned(),
            }),
        },
    }
}

fn help(msg: impl Into<String>) -> tola_diagnostic::Help {
    tola_diagnostic::Help {
        message: msg.into(),
        location: None,
    }
}

fn owned_namespace_guidance(pkg: &str) -> Option<PackageFailureGuidance> {
    let spec = pkg.parse::<typst::syntax::package::PackageSpec>().ok()?;
    if spec.namespace.as_str() != tola_packages::TOLA_NAMESPACE {
        return None;
    }
    let provided = tola_packages::TolaPackage::all()
        .iter()
        .copied()
        .find(|cand| cand.name() == spec.name.as_str());
    Some(match provided {
        Some(provided) => PackageFailureGuidance {
            message: format!("Tola provides `{}`, not `{pkg}`", provided.spec()),
            note: None,
            help: help(format!("Import `{}`", provided.spec())),
        },
        None => PackageFailureGuidance {
            message: format!("`{pkg}` is not a Tola package"),
            note: None,
            help: help("Check the package name"),
        },
    })
}

fn with_file_failure_reason(diag: Diagnostic, err: &typst::diag::FileError) -> Diagnostic {
    match file_failure_reason(err) {
        Some(reason) => diag.with_note(reason),
        None => diag,
    }
}

fn imported_by_note(files: &[String]) -> Option<String> {
    (!files.is_empty()).then(|| {
        format!(
            "package references in {}",
            tola_diagnostic::bounded_listing(files, ("file", "files"))
        )
    })
}

pub(crate) fn warning_diagnostics(
    config: &crate::config::SiteConfig,
    warnings: &mut Diagnostics,
) -> Vec<Diagnostic> {
    let retained = std::mem::take(warnings);
    let warnings = without_export_warnings(&retained);
    convert_reports(config.root(), &warnings)
}
