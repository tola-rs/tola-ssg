//! Formatting and collection of Typst diagnostics.

use std::path::Path;
use tola_typst::{DiagnosticFilter, Diagnostics};

use crate::diagnostic::{Diagnostic, DiagnosticCause, Severity};
use crate::filesystem::root_relative;

/// Convert Typst compiler failures into shared diagnostics.
///
/// Presentation, display limits, and terminal policy belong to consumers.
pub(crate) fn error_diagnostics(
    error: &anyhow::Error,
    root: &Path,
) -> Option<Vec<crate::diagnostic::Diagnostic>> {
    let compile = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tola_typst::CompileError>())?;
    compile_failure_diagnostics(compile, root)
}

/// Classify one Typst compiler failure into diagnostics the site author reads.
///
/// Returns `None` for cancellation, which callers report as cancellation instead of failure.
pub(super) fn compile_failure_diagnostics(
    compile: &tola_typst::CompileError,
    root: &Path,
) -> Option<Vec<crate::diagnostic::Diagnostic>> {
    if let Some(diagnostics) = compile.diagnostics() {
        let diagnostics = diagnostics
            .filter_out(&DiagnosticFilter::typst_export_warning_filters())
            .iter()
            .map(|diagnostic| {
                resolved_diagnostic(diagnostic.clone(), root, crate::codes::typst::COMPILE)
            })
            .collect::<Vec<_>>();
        return Some(ensure_error_diagnostic(
            diagnostics,
            crate::codes::typst::COMPILE,
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
                let source_diagnostic = source_diagnostic.source();
                let mut message = source_diagnostic.message.to_string();
                relativize_message(&mut message, root);
                let mut diagnostic = crate::diagnostic::Diagnostic::new(
                    crate::codes::typst::BUNDLE_EXPORT,
                    match source_diagnostic.severity {
                        typst::diag::Severity::Error => crate::diagnostic::Severity::Error,
                        typst::diag::Severity::Warning => crate::diagnostic::Severity::Warning,
                    },
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
            crate::codes::typst::BUNDLE_EXPORT,
            "Tola could not export the Bundle",
        ));
    }
    let diagnostic = match compile {
        tola_typst::CompileError::HtmlExport { message } => Diagnostic::new(
            crate::codes::typst::HTML_EXPORT,
            Severity::Error,
            relativized_text(message, root),
        ),
        tola_typst::CompileError::Input { message } => Diagnostic::new(
            crate::codes::typst::INPUT,
            Severity::Error,
            relativized_text(message, root),
        ),
        tola_typst::CompileError::Snapshot(error) => snapshot_diagnostic(error, root)?,
        tola_typst::CompileError::Font(error) => font_diagnostic(error, root)?,
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

/// One snapshot failure in the site author's words, or `None` for cancellation.
///
/// `CompileError::from` already folds a cancelled snapshot into `CompileError::Cancelled`, so
/// a cancelled snapshot answers `None` here and the caller reports cancellation instead.
fn snapshot_diagnostic(
    error: &tola_typst::world::SnapshotError,
    root: &Path,
) -> Option<Diagnostic> {
    use tola_typst::world::SnapshotError;
    Some(match error {
        SnapshotError::Cancelled => return None,
        SnapshotError::File { path, error } => match site_relative(path, root) {
            Some(relative) => {
                let diagnostic = Diagnostic::new(
                    crate::codes::typst::SNAPSHOT,
                    Severity::Error,
                    format!("Tola could not read `{relative}`"),
                )
                .with_path(relative)
                .with_help("Check that the file exists and is readable, then run the build again");
                match file_failure_reason(error) {
                    Some(reason) => diagnostic.with_note(reason),
                    None => diagnostic,
                }
            }
            None => Diagnostic::new(
                crate::codes::typst::SNAPSHOT,
                Severity::Error,
                "Tola could not read a source this build needs",
            )
            .with_help("Check that each content source exists inside the site root"),
        },
        SnapshotError::OutsideRoot { .. } => Diagnostic::new(
            crate::codes::typst::SNAPSHOT,
            Severity::Error,
            "a source this build reads is outside the site root",
        )
        .with_help("Keep the content root and the Bundle entry inside the site root"),
        SnapshotError::RootMismatch { .. } => Diagnostic::new(
            crate::codes::typst::SNAPSHOT,
            Severity::Error,
            "Tola could not reuse the sources it read before the site root changed",
        )
        .with_help("Restart `tola dev` to read the sources again"),
    })
}

/// One font-resource failure in the site author's words, or `None` for cancellation.
///
/// `CompileError::from` already folds a cancelled font load into `CompileError::Cancelled`, so
/// a cancelled load answers `None` here and the caller reports cancellation instead.
fn font_diagnostic(error: &tola_typst::FontLoadError, root: &Path) -> Option<Diagnostic> {
    use tola_typst::FontLoadError;
    Some(match error {
        FontLoadError::Cancelled => return None,
        FontLoadError::File { path, error } => match site_relative(path, root) {
            Some(relative) => {
                let diagnostic = Diagnostic::new(
                    crate::codes::typst::FONT,
                    Severity::Error,
                    format!("Tola could not read the font file `{relative}`"),
                )
                .with_path(relative)
                .with_help("Check the font file, or remove it from `typst.fonts.paths`");
                match file_failure_reason(error) {
                    Some(reason) => diagnostic.with_note(reason),
                    None => diagnostic,
                }
            }
            None => Diagnostic::new(
                crate::codes::typst::FONT,
                Severity::Error,
                "Tola could not read a font file this site configures",
            )
            .with_help("Check the `typst.fonts.paths` directories"),
        },
        FontLoadError::Changed { path } => match site_relative(path, root) {
            Some(relative) => Diagnostic::new(
                crate::codes::typst::FONT,
                Severity::Error,
                format!("the font file `{relative}` changed while Tola was loading fonts"),
            )
            .with_path(relative)
            .with_help("Run the build again without changing font files"),
            None => Diagnostic::new(
                crate::codes::typst::FONT,
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
            crate::codes::typst::WORLD,
            Severity::Error,
            "the Bundle entry is outside the site root",
        )
        .with_help("Set `build.entry` to a file inside the site root"),
        WorldBuildError::MissingFileCache => Diagnostic::new(
            crate::codes::typst::WORLD,
            Severity::Error,
            "the site sources could not be loaded for this build",
        )
        .with_help("Restart `tola dev` and run the build again"),
        WorldBuildError::MissingFonts => Diagnostic::new(
            crate::codes::typst::WORLD,
            Severity::Error,
            "the configured font files could not be loaded for this build",
        )
        .with_help("Restart `tola dev` and run the build again"),
        WorldBuildError::SnapshotRootMismatch { .. } => Diagnostic::new(
            crate::codes::typst::WORLD,
            Severity::Error,
            "the site root changed while Tola was reading sources",
        )
        .with_help("Restart `tola dev` to read the sources again"),
        WorldBuildError::FixedTimeWithoutDate => Diagnostic::new(
            crate::codes::typst::WORLD,
            Severity::Error,
            "the build date has no calendar day",
        )
        .with_help("Give the build a date that includes its year, month, and day"),
        WorldBuildError::Source(error) => Diagnostic::new(
            crate::codes::typst::WORLD,
            Severity::Error,
            error.to_string(),
        ),
        WorldBuildError::Font(error) => font_diagnostic(error, root)?,
    })
}

/// The site-author reason behind one Typst file failure, when a plain one exists.
///
/// Details Typst keeps for its own diagnostics — conflicting editor versions, unmapped paths —
/// stay out of the rendered reason.
fn file_failure_reason(error: &typst::diag::FileError) -> Option<String> {
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
    fallback_code: crate::diagnostic::DiagnosticCode,
) -> anyhow::Error {
    if crate::cancellation::is_cancelled(&error) || crate::diagnostic::attached(&error).is_some() {
        return error;
    }
    let diagnostics = error_diagnostics(&error, root)
        .unwrap_or_else(|| vec![crate::diagnostic::fallback(fallback_code, &error)]);
    anyhow::Error::new(crate::diagnostic::DiagnosticError::attach(
        error,
        diagnostics,
    ))
}

pub(super) fn with_export_diagnostics(
    error: tola_typst::CompileError,
    world: &tola_typst::TypstWorld,
    compilation: &tola_typst::BundleCompilation,
    root: &Path,
) -> anyhow::Error {
    let Some(raw) = error.raw_diagnostics() else {
        return with_diagnostics(error.into(), root, crate::codes::typst::SITE_PROGRAM);
    };
    let mut diagnostics = Diagnostics::resolve(world, raw);
    diagnostics.attach_imported_by(|package| compilation.files_importing(package).to_vec());
    let diagnostics = diagnostics
        .filter_out(&DiagnosticFilter::typst_export_warning_filters())
        .into_iter()
        .map(|diagnostic| resolved_diagnostic(diagnostic, root, crate::codes::typst::BUNDLE_EXPORT))
        .collect();
    let diagnostics = ensure_error_diagnostic(
        diagnostics,
        crate::codes::typst::BUNDLE_EXPORT,
        "Tola could not export the Bundle",
    );
    crate::diagnostic::DiagnosticError::attach(error.into(), diagnostics).into()
}

fn ensure_error_diagnostic(
    mut diagnostics: Vec<Diagnostic>,
    code: crate::diagnostic::DiagnosticCode,
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
///
/// A path outside the root is not rendered, so callers state the failure without one.
pub(super) fn site_relative(path: &Path, root: &Path) -> Option<String> {
    let relative = root_relative(path, root);
    (!relative.as_os_str().is_empty() && relative != path)
        .then(|| crate::filesystem::display_path(path, root))
}

pub(super) fn resolved_diagnostic(
    mut diagnostic: tola_typst::Diagnostic,
    root: &Path,
    code: crate::diagnostic::DiagnosticCode,
) -> crate::diagnostic::Diagnostic {
    let code = if tola_packages::AssetUrlShadowed::from_origin(&diagnostic.origin).is_some() {
        crate::codes::reference::ASSET_URL_SHADOWED
    } else {
        code
    };
    relativize_diagnostic(&mut diagnostic, root);
    let severity = match diagnostic.severity {
        tola_typst::DiagnosticSeverity::Error => crate::diagnostic::Severity::Error,
        tola_typst::DiagnosticSeverity::Warning => crate::diagnostic::Severity::Warning,
    };
    let mut notes = Vec::new();
    if let Some(note) = imported_by_note(&diagnostic.imported_by) {
        notes.push(note);
    }
    let mut help = Vec::new();
    if let Some(failure) = &diagnostic.package_failure {
        let guidance = package_failure_guidance(failure, root);
        diagnostic.message = guidance.message;
        notes.extend(guidance.note);
        help.extend(guidance.help);
    }
    let location = diagnostic
        .location
        .path
        .as_ref()
        .map(|_| diagnostic_location(&diagnostic.location));
    let cause = unknown_variable_cause(&diagnostic.message, location.as_ref());
    crate::diagnostic::Diagnostic {
        severity,
        code,
        message: diagnostic.message,
        location,
        imported_by: diagnostic.imported_by,
        notes,
        help: help
            .into_iter()
            .chain(diagnostic.hints.into_iter().map(help_from_hint))
            .collect(),
        trace: diagnostic.traces.into_iter().map(trace_frame).collect(),
        cause,
    }
}

/// The name the highlighted source text leaves unbound, or `None` when no highlight names one.
///
/// Typst's diagnostics hold no error kind, so the compiler's own wording is the only signal that
/// the fault is an unbound name; the name is read from the highlight rather than from the sentence.
/// A wording change drops the cause instead of producing a wrong one.
fn unknown_variable_cause(
    message: &str,
    location: Option<&crate::diagnostic::Location>,
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
    help: Option<crate::diagnostic::Help>,
}

/// The package directory a site declares is the place the author can commit, so it
/// leads; a package kept elsewhere is reached with `--package-path`. Directories
/// outside the site root stay unnamed.
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
                help: Some(help(match directory {
                    Some(directory) => format!("Put the package in `{directory}/`"),
                    None => "Run `tola vendor` to add the package to the site".to_owned(),
                })),
            },
        },
        ResolvedPackageFailureReason::Unavailable => PackageFailureGuidance {
            message: format!("Tola could not install the package `{package}`"),
            note: None,
            help: Some(help(match directory {
                Some(directory) => {
                    format!("Add it to `{directory}/` so the site includes the package itself")
                }
                None => "Make the package cache writable".to_owned(),
            })),
        },
    }
}

fn help(message: impl Into<String>) -> crate::diagnostic::Help {
    crate::diagnostic::Help {
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
            help: Some(help(format!("Import `{}`", provided.spec()))),
        },
        None => PackageFailureGuidance {
            message: format!("`{package}` is not a Tola package"),
            note: None,
            help: Some(help(
                "Check the package name; run `tola skill` to see the packages Tola provides",
            )),
        },
    })
}

fn source_line(source: tola_typst::SourceLine) -> crate::diagnostic::SourceLine {
    let highlight = source
        .highlight
        .and_then(|range| char_range_to_byte_range(&source.text, range));
    crate::diagnostic::SourceLine {
        start_column: source.start_column,
        start_character: source.start_character,
        ends_line: source.ends_line,
        ..crate::diagnostic::SourceLine::new(source.line_num, source.text, highlight)
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

fn diagnostic_location(source: &tola_typst::SourceLocation) -> crate::diagnostic::Location {
    crate::diagnostic::Location {
        path: source.path.clone().unwrap_or_default(),
        line: source.line,
        column: source.column,
        range: source.range.map(|range| crate::diagnostic::SourceRange {
            start: crate::diagnostic::SourcePosition {
                line: range.start.line,
                character: range.start.character,
            },
            end: crate::diagnostic::SourcePosition {
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
) -> Option<crate::diagnostic::Location> {
    tola_typst::diagnostic::resolve_source_location(world, span)
        .map(|source| diagnostic_location(&source))
}

fn help_from_hint(hint: tola_typst::Hint) -> crate::diagnostic::Help {
    crate::diagnostic::Help {
        message: hint.message,
        location: hint
            .location
            .path
            .as_ref()
            .map(|_| diagnostic_location(&hint.location)),
    }
}

fn trace_frame(trace: tola_typst::Trace) -> crate::diagnostic::TraceFrame {
    crate::diagnostic::TraceFrame {
        message: trace.message,
        location: trace
            .location
            .path
            .as_ref()
            .map(|_| diagnostic_location(&trace.location)),
    }
}

/// Static source navigation, not a claim that each import or include executed.
fn imported_by_note(files: &[String]) -> Option<String> {
    (!files.is_empty()).then(|| {
        format!(
            "package references in {}",
            crate::diagnostic::bounded_listing(files, ("file", "files"))
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

/// Rewrite every site-root prefix inside `text` into its site-relative spelling.
///
/// Typst names the files it searched with host-absolute paths; the author reads paths relative to
/// their site, with `/` separators.
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

/// The path a matched site root introduces, when the match really names that root.
///
/// A root that continues or extends a longer component — a sibling directory, or the site root
/// named without a path after it — belongs to the surrounding sentence, not to the site.
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
) -> Vec<crate::diagnostic::Diagnostic> {
    let warnings =
        std::mem::take(warnings).filter_out(&DiagnosticFilter::typst_export_warning_filters());
    warnings
        .iter()
        .map(|warning| {
            resolved_diagnostic(
                warning.clone(),
                config.get_root(),
                crate::codes::typst::COMPILE,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn package_failure_names_the_site_directory() {
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
        type Case<'a> = (
            &'a str,
            ResolvedPackageFailureReason,
            Vec<ResolvedPackageSearch>,
            &'a str,
            Option<&'a str>,
            &'a [&'a str],
        );
        let cases: [Case<'_>; 4] = [
            (
                "@cawemo/report:1.2.0",
                ResolvedPackageFailureReason::NotInstalled,
                vec![
                    site_packages("cawemo/report", "1.2.0"),
                    host_cache("cawemo/report", "1.2.0"),
                ],
                "the package `@cawemo/report:1.2.0` is not installed",
                Some("`packages/cawemo/report/1.2.0` does not exist"),
                &["`packages/`"],
            ),
            (
                "@preview/cetz:0.3.4",
                ResolvedPackageFailureReason::Unavailable,
                vec![
                    site_packages("preview/cetz", "0.3.4"),
                    host_cache("preview/cetz", "0.3.4"),
                ],
                "Tola could not install the package `@preview/cetz:0.3.4`",
                None,
                &["`packages/`"],
            ),
            (
                "@preview/cetz:0.3.4",
                ResolvedPackageFailureReason::NotInstalled,
                vec![host_cache("preview/cetz", "0.3.4")],
                "the package `@preview/cetz:0.3.4` is not installed",
                None,
                &["tola vendor"],
            ),
            (
                "@preview/cetz:0.3.4",
                ResolvedPackageFailureReason::Unavailable,
                vec![host_cache("preview/cetz", "0.3.4")],
                "Tola could not install the package `@preview/cetz:0.3.4`",
                None,
                &["cache"],
            ),
        ];

        for (package, reason, searched, message, note, help_fragments) in &cases {
            let failure = ResolvedPackageFailure {
                package: (*package).into(),
                reason: *reason,
                searched: searched.clone(),
            };

            let guidance = package_failure_guidance(&failure, root);

            assert_eq!(guidance.message, *message, "{package} {reason:?}");
            assert_eq!(guidance.note.as_deref(), *note, "{package} {reason:?}");
            let help = guidance.help.expect("the author learns what to do");
            for &fragment in *help_fragments {
                assert!(help.message.contains(fragment), "{}", help.message);
            }
            assert!(!help.message.contains("author"), "{}", help.message);
        }
    }

    #[test]
    fn display_limits_do_not_drop_warnings() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let make_warning = |message: &str| warning_at(message, location(None));
        let mut warnings = Diagnostics::new();
        warnings.extend_distinct(&Diagnostics::from_vec(vec![
            make_warning("first warning"),
            make_warning("second warning"),
        ]));
        let diagnostics = warning_diagnostics(&config, &mut warnings);
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>(),
            ["first warning", "second warning"]
        );
    }

    #[test]
    fn export_failure_keeps_its_message() {
        let raw = typst::diag::SourceDiagnostic::error(typst::syntax::Span::detached(), "broken")
            .with_hint("choose another value");
        let error = anyhow::Error::new(tola_typst::CompileError::bundle_export([raw.into()]));

        let diagnostics = error_diagnostics(&error, Path::new("/site")).unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, crate::codes::typst::BUNDLE_EXPORT);
        assert_eq!(diagnostics[0].message, "broken");
        assert_eq!(diagnostics[0].help[0].message, "choose another value");

        let empty = anyhow::Error::new(tola_typst::CompileError::bundle_export([]));
        let diagnostics = error_diagnostics(&empty, Path::new("/site")).unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, crate::codes::typst::BUNDLE_EXPORT);
    }

    #[test]
    fn warning_only_failure_reports_error() {
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
            assert_eq!(diagnostics[0].severity, crate::diagnostic::Severity::Error);
            assert_eq!(
                diagnostics[1].severity,
                crate::diagnostic::Severity::Warning
            );
        }
    }

    #[test]
    fn experimental_warning_yields_export_error() {
        let warning = typst::diag::SourceDiagnostic::warning(
            typst::syntax::Span::detached(),
            "bundle export is experimental",
        );
        let error = anyhow::Error::new(tola_typst::CompileError::bundle_export([warning.into()]));

        let diagnostics = error_diagnostics(&error, Path::new("/site")).unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, crate::diagnostic::Severity::Error);
    }

    #[test]
    fn snapshot_failure_names_the_file_itself() {
        let failure = |error: typst::diag::FileError| {
            anyhow::Error::new(tola_typst::CompileError::Snapshot(
                tola_typst::world::SnapshotError::File {
                    path: "/site/content/post.typ".into(),
                    error,
                },
            ))
        };

        let opaque = error_diagnostics(
            &failure(typst::diag::FileError::Other(Some(
                "file disappeared".into(),
            ))),
            Path::new("/site"),
        )
        .unwrap();

        assert_eq!(opaque[0].code, crate::codes::typst::SNAPSHOT);
        assert_eq!(opaque[0].message, "Tola could not read `content/post.typ`");
        assert_eq!(
            opaque[0].location.as_ref().unwrap().path,
            "content/post.typ"
        );
        assert!(opaque[0].notes.is_empty());

        let explained = error_diagnostics(
            &failure(typst::diag::FileError::AccessDenied),
            Path::new("/site"),
        )
        .unwrap();

        assert_eq!(
            explained[0].message,
            "Tola could not read `content/post.typ`"
        );
        assert_eq!(explained[0].notes, ["Tola is not allowed to read it"]);
    }

    #[test]
    fn font_failure_names_the_font_file() {
        let error = anyhow::Error::new(tola_typst::CompileError::Font(
            tola_typst::FontLoadError::File {
                path: "/site/static/typst-fonts/Inter.ttf".into(),
                error: typst::diag::FileError::NotFound(
                    "/site/static/typst-fonts/Inter.ttf".into(),
                ),
            },
        ));

        let diagnostics = error_diagnostics(&error, Path::new("/site")).unwrap();

        assert_eq!(diagnostics[0].code, crate::codes::typst::FONT);
        assert_eq!(
            diagnostics[0].message,
            "Tola could not read the font file `static/typst-fonts/Inter.ttf`"
        );
        assert_eq!(diagnostics[0].notes, ["it does not exist"]);
        assert_eq!(
            diagnostics[0].location.as_ref().unwrap().path,
            "static/typst-fonts/Inter.ttf"
        );
    }

    #[test]
    fn world_failure_names_no_outside_path() {
        let error = anyhow::Error::new(tola_typst::CompileError::World(
            tola_typst::WorldBuildError::MainOutsideRoot {
                main: "/other/site.typ".into(),
                root: "/site".into(),
            },
        ));

        let diagnostics = error_diagnostics(&error, Path::new("/site")).unwrap();

        assert_eq!(diagnostics[0].code, crate::codes::typst::WORLD);
        assert_eq!(
            diagnostics[0].message,
            "the Bundle entry is outside the site root"
        );
        assert_eq!(
            diagnostics[0].help[0].message,
            "Set `build.entry` to a file inside the site root"
        );
        assert!(diagnostics[0].location.is_none());
    }

    #[test]
    fn site_root_prefixes_become_relative() {
        let diagnostic = warning(
            "file not found (searched at /site/templates/missing.txt)",
            "file not found (searched at /site/content/post.typ)",
        );

        let diagnostic =
            resolved_diagnostic(diagnostic, Path::new("/site"), crate::codes::typst::COMPILE);

        assert_eq!(
            diagnostic.message,
            "file not found (searched at templates/missing.txt)"
        );
        assert_eq!(
            diagnostic.help[0].message,
            "file not found (searched at content/post.typ)"
        );

        let mut outside = String::from("cannot read /other/site/content/post.typ");
        relativize_message(&mut outside, Path::new("/site"));
        assert_eq!(outside, "cannot read /other/site/content/post.typ");
        let mut sibling = String::from("cannot read /site-sibling/post.typ");
        relativize_message(&mut sibling, Path::new("/site"));
        assert_eq!(sibling, "cannot read /site-sibling/post.typ");
    }

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

    fn warning(message: &str, help: &str) -> tola_typst::Diagnostic {
        tola_typst::Diagnostic {
            hints: vec![tola_typst::Hint {
                message: help.into(),
                location: location(None),
            }],
            ..warning_at(message, location(None))
        }
    }

    #[test]
    fn character_highlights_become_byte_ranges() {
        let line = source_line(tola_typst::SourceLine {
            line_num: 1,
            start_column: 0,
            start_character: 0,
            ends_line: true,
            text: "éx".into(),
            highlight: Some((1, 2)),
        });

        assert_eq!(line.highlight, Some((2, 3)));
    }

    #[test]
    fn converted_paths_are_site_relative() {
        let hint = tola_typst::SourceLocation {
            line: Some(1),
            column: Some(1),
            truncation: tola_typst::SourceTruncation {
                omitted_lines: 1,
                omitted_bytes: 2,
            },
            ..location(Some("/site/templates/document.typ"))
        };
        let resolved = tola_typst::Diagnostic {
            hints: vec![tola_typst::Hint {
                message: "inspect the import".into(),
                location: hint,
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

        let diagnostic =
            resolved_diagnostic(resolved, Path::new("/site"), crate::codes::typst::COMPILE);

        assert_eq!(diagnostic.code, "typst.compile");
        assert_eq!(diagnostic.severity, crate::diagnostic::Severity::Warning);
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

    #[test]
    fn unknown_variable_names_its_highlighted_name() {
        let diagnostic = tola_typst::Diagnostic {
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

        let resolved =
            resolved_diagnostic(diagnostic, Path::new("/site"), crate::codes::typst::COMPILE);

        assert_eq!(
            resolved.cause,
            Some(crate::diagnostic::DiagnosticCause::UnknownVariable {
                name: "absent-name".to_owned(),
            })
        );
    }

    #[test]
    fn failures_naming_no_unbound_variable_have_no_cause() {
        let unknown_without_highlight = tola_typst::Diagnostic {
            origin: tola_typst::DiagnosticOrigin::Typst,
            severity: tola_typst::DiagnosticSeverity::Error,
            message: "unknown variable: absent-name".into(),
            location: location(Some("/site/content/index.typ")),
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        };
        let another_failure = tola_typst::Diagnostic {
            origin: tola_typst::DiagnosticOrigin::Typst,
            message: "expected expression".into(),
            location: tola_typst::SourceLocation {
                line: Some(1),
                column: Some(1),
                source_lines: vec![tola_typst::SourceLine {
                    line_num: 1,
                    start_column: 0,
                    start_character: 0,
                    ends_line: true,
                    text: ")".into(),
                    highlight: Some((0, 1)),
                }],
                ..location(Some("/site/content/index.typ"))
            },
            ..unknown_without_highlight.clone()
        };

        let panic = tola_typst::Diagnostic {
            message: "panicked with: unknown variable: absent-name".into(),
            ..another_failure.clone()
        };
        for diagnostic in [unknown_without_highlight, another_failure, panic] {
            let resolved =
                resolved_diagnostic(diagnostic, Path::new("/site"), crate::codes::typst::COMPILE);
            assert_eq!(resolved.cause, None, "{resolved:?}");
        }
    }

    #[test]
    fn clipped_identifier_offers_no_correction() {
        let location = crate::diagnostic::Location {
            path: "content/page.typ".into(),
            line: Some(1),
            column: Some(101),
            range: Some(crate::diagnostic::SourceRange {
                start: crate::diagnostic::SourcePosition {
                    line: 0,
                    character: 100,
                },
                end: crate::diagnostic::SourcePosition {
                    line: 0,
                    character: 111,
                },
            }),
            source_lines: vec![crate::diagnostic::SourceLine {
                start_column: 100,
                start_character: 100,
                ..crate::diagnostic::SourceLine::new(1, "absent", Some((0, 6)))
            }],
        };
        assert_eq!(
            unknown_variable_cause("unknown variable: absent-name", Some(&location)),
            None
        );
    }

    #[test]
    fn bundle_feature_warning_is_hidden() {
        let warning = warning_at("bundle export is experimental", location(None));

        let filtered = Diagnostics::from_vec(vec![warning])
            .filter_out(&DiagnosticFilter::typst_export_warning_filters());

        assert!(filtered.is_empty());
    }
}
