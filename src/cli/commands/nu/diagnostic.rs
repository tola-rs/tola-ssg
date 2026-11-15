//! Submit Nu reports and render standalone diagnostics.

use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;
use tola_build::report::{
    DEFAULT_MAX_REPORT_BYTES, REPORT_SCHEMA_VERSION, ReportBatch, ReportSources,
};
use tola_diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticProducer, Help, Location, RelatedInformation, Severity,
    SourceRange,
};

use crate::terminal::{SourceFiles, render_diagnostic};

const ALREADY_SUBMITTED: &str = "this invocation already submitted a diagnostic report";

type SubmittedDiagnostics = Arc<Mutex<Option<Vec<Diagnostic>>>>;

pub(in crate::cli::commands) fn collector(
    producer: String,
    sources: ReportSources,
) -> (tola_nu::SubmitDiagnostics, SubmittedDiagnostics) {
    let diagnostics = Arc::new(Mutex::new(None));
    let collected = diagnostics.clone();
    let submit: tola_nu::SubmitDiagnostics = Arc::new(move |issues| {
        let mut submitted = collected.lock().expect("diagnostic collector");
        if submitted.is_some() {
            return Err(ALREADY_SUBMITTED.into());
        }
        let issues = report_batch(issues)?
            .validate(&producer, &sources)
            .map_err(|error| error.to_string())?;
        *submitted = Some(issues);
        Ok(())
    });
    (submit, diagnostics)
}

fn report_batch(issues: Value) -> Result<ReportBatch, String> {
    Ok(ReportBatch {
        schema_version: REPORT_SCHEMA_VERSION,
        diagnostics: serde_json::from_value(issues).map_err(|error| error.to_string())?,
    })
}

pub(super) fn submit_file(path: &Path, issues: Value) -> Result<(), String> {
    if path.exists() {
        return Err(ALREADY_SUBMITTED.into());
    }
    let batch = report_batch(issues)?;
    let bytes = serde_json::to_vec(&batch).map_err(|error| error.to_string())?;
    if bytes.len() > DEFAULT_MAX_REPORT_BYTES {
        return Err(format!(
            "diagnostic report exceeds the {DEFAULT_MAX_REPORT_BYTES}-byte limit"
        ));
    }
    batch
        .validate("Nu script", &ReportSources::new())
        .map_err(|error| error.to_string())?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let write_error = |_| "could not write the diagnostic report".to_owned();
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(write_error)?;
    file.write_all(&bytes).map_err(write_error)?;
    file.persist_noclobber(path).map_err(|error| {
        if error.error.kind() == ErrorKind::AlreadyExists {
            ALREADY_SUBMITTED.into()
        } else {
            write_error(error.error)
        }
    })?;
    Ok(())
}

#[derive(Deserialize)]
struct Issue {
    severity: String,
    message: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    location: Option<IssueLocation>,
    #[serde(default)]
    hints: Vec<String>,
    #[serde(default)]
    help: Vec<IssueRelated>,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default)]
    related: Vec<IssueRelated>,
}

#[derive(Deserialize)]
struct IssueRelated {
    message: String,
    #[serde(default)]
    location: Option<IssueLocation>,
}

#[derive(Deserialize)]
struct IssueLocation {
    path: Option<String>,
    #[serde(default)]
    line: Option<usize>,
    #[serde(default)]
    column: Option<usize>,
    #[serde(default)]
    range: Option<SourceRange>,
}

#[derive(Debug, thiserror::Error)]
enum RenderInputError {
    #[error("expected a list of diagnostics")]
    List,
    #[error("invalid diagnostic {number}: {source}")]
    Issue {
        number: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "diagnostic {number} has severity `{severity}`, expected `error`, `warning`, `info`, or `hint`"
    )]
    Severity { number: usize, severity: String },
    #[error(
        "diagnostic {number} needs a file `path` in `{field}` for rendering, while `source` refers to a host snapshot"
    )]
    MissingPath { number: usize, field: String },
}

pub(super) fn render(directory: &Path, input: Value, color: bool) -> anyhow::Result<String> {
    let diagnostics = parse_diagnostics(&input)?;
    if diagnostics.is_empty() {
        return Ok(String::new());
    }
    let sources = SourceFiles::new(
        directory.to_owned(),
        tola_typst::PackageLocations::discover(None, None)
            .context("cannot find local Typst packages for diagnostic source text")?,
    );
    let mut text = String::new();
    for diagnostic in diagnostics {
        text.push_str(&render_diagnostic(&diagnostic, Some(&sources), color));
        text.push('\n');
    }
    Ok(text)
}

fn parse_diagnostics(input: &Value) -> Result<Vec<Diagnostic>, RenderInputError> {
    let issues = input.as_array().ok_or(RenderInputError::List)?;
    issues
        .iter()
        .enumerate()
        .map(|(index, issue)| {
            let number = index + 1;
            let issue = Issue::deserialize(issue)
                .map_err(|source| RenderInputError::Issue { number, source })?;
            into_diagnostic(issue, number)
        })
        .collect()
}

fn into_diagnostic(issue: Issue, number: usize) -> Result<Diagnostic, RenderInputError> {
    let severity = match issue.severity.as_str() {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        "info" => Severity::Info,
        "hint" => Severity::Hint,
        _ => {
            return Err(RenderInputError::Severity {
                number,
                severity: issue.severity,
            });
        }
    };
    // The public code is carried by the producer; the static code only satisfies the core type.
    let mut diagnostic = Diagnostic::new(DiagnosticCode::new("nu.issue"), severity, issue.message);
    diagnostic.producer = Some(DiagnosticProducer {
        name: Some(issue.code.unwrap_or_else(|| "nu.issue".to_owned())),
        code: None,
    });
    let mut help = issue
        .help
        .into_iter()
        .enumerate()
        .map(|(index, help)| {
            let location = help
                .location
                .map(|location| {
                    into_location(location, number, || format!("help[{index}].location"))
                })
                .transpose()?;
            Ok(Help {
                message: help.message,
                location,
            })
        })
        .collect::<Result<Vec<_>, RenderInputError>>()?;
    help.extend(issue.hints.into_iter().map(|message| Help {
        message,
        location: None,
    }));
    diagnostic.help = help;
    diagnostic.notes = issue.notes;
    diagnostic.related = issue
        .related
        .into_iter()
        .enumerate()
        .map(|(index, related)| {
            let location = related
                .location
                .map(|location| {
                    into_location(location, number, || format!("related[{index}].location"))
                })
                .transpose()?;
            Ok(RelatedInformation {
                message: related.message,
                location,
                source: None,
            })
        })
        .collect::<Result<_, RenderInputError>>()?;
    diagnostic.location = issue
        .location
        .map(|location| into_location(location, number, || "location".into()))
        .transpose()?;
    Ok(diagnostic)
}

fn into_location(
    location: IssueLocation,
    number: usize,
    field: impl FnOnce() -> String,
) -> Result<Location, RenderInputError> {
    let path = location
        .path
        .filter(|path| !path.is_empty())
        .ok_or_else(|| RenderInputError::MissingPath {
            number,
            field: field(),
        })?;
    Ok(Location {
        file: None,
        path,
        line: location.line,
        column: location.column,
        range: location.range,
        source_lines: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn embedded_submission_keeps_the_first_valid_batch() {
        let (submit, diagnostics) = collector("test".into(), ReportSources::new());
        assert!(submit(json!([{"severity":"warning","message":" "}])).is_err());
        submit(json!([{"severity":"error","message":"missing title"}])).unwrap();
        assert!(submit(json!([])).is_err());
        let diagnostics = diagnostics.lock().unwrap();
        let first = diagnostics.as_ref().unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].severity, Severity::Error);
    }

    #[test]
    fn an_empty_file_submission_still_counts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        assert!(submit_file(&path, json!([{"severity":"warning","message":" "}])).is_err());
        assert!(!path.exists());
        submit_file(&path, json!([])).unwrap();
        assert!(
            submit_file(
                &path,
                json!([{"severity":"error","message":"missing title"}])
            )
            .is_err()
        );
        let batch = ReportBatch::read_optional(&path, DEFAULT_MAX_REPORT_BYTES)
            .unwrap()
            .unwrap();
        assert!(batch.diagnostics.is_empty());
    }

    #[test]
    fn source_snapshot_location_is_rejected_by_rendering() {
        let input = json!([
            {"severity":"warning","message":"missing summary"},
            {"severity":"error","message":"missing title","related":[
                {"message":"declared here","location":{"source":"guide"}}
            ]}
        ]);
        assert!(matches!(
            parse_diagnostics(&input),
            Err(RenderInputError::MissingPath { number: 2, field })
                if field == "related[0].location"
        ));
    }

    #[test]
    fn render_keeps_suggestions_from_check_reports() {
        let mut issue = Diagnostic::new(
            DiagnosticCode::new("test.issue"),
            Severity::Warning,
            "Missing title",
        );
        issue.help.push(Help {
            message: "Add a title here".into(),
            location: Some(Location {
                file: None,
                path: "content/index.typ".into(),
                line: Some(2),
                column: None,
                range: None,
                source_lines: Vec::new(),
            }),
        });
        let parsed = parse_diagnostics(&json!([issue])).unwrap();
        assert_eq!(parsed[0].help, issue.help);
    }
}
