//! Render diagnostic JSON through the renderer builds use.
//!
//! Read a list of diagnostics or an object with a diagnostics array. Locations name files.

use std::io::Read;

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;
use tola_diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticProducer, Help, Location, RelatedInformation, Severity,
    SourceRange,
};

use crate::cli::output::CommandOutput;

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

#[derive(Deserialize)]
#[serde(untagged)]
enum Input {
    List(Vec<Value>),
    Object { diagnostics: Vec<Value> },
}

#[derive(Debug, thiserror::Error)]
enum RenderInputError {
    #[error("expected a JSON array of diagnostics or an object with a `diagnostics` array")]
    Json(#[source] serde_json::Error),
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

pub(in crate::cli) fn render(output: &CommandOutput) -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("read diagnostic JSON from stdin")?;
    let diagnostics = parse_diagnostics(&input)?;
    if diagnostics.is_empty() {
        return Ok(());
    }
    let sources = crate::terminal::SourceFiles::new(
        std::env::current_dir()?,
        tola_typst::PackageLocations::discover(None, None)?,
    );
    for diagnostic in diagnostics {
        println!(
            "{}",
            crate::terminal::render_diagnostic(
                &diagnostic,
                Some(&sources),
                output.terminal().stdout_uses_color(),
            )
        );
    }
    Ok(())
}

fn parse_diagnostics(input: &str) -> Result<Vec<Diagnostic>, RenderInputError> {
    let issues = match serde_json::from_str::<Input>(input).map_err(RenderInputError::Json)? {
        Input::List(issues)
        | Input::Object {
            diagnostics: issues,
        } => issues,
    };
    issues
        .into_iter()
        .enumerate()
        .map(|(index, issue)| {
            let number = index + 1;
            let issue = serde_json::from_value(issue)
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
    diagnostic.help = issue
        .hints
        .into_iter()
        .map(|message| Help {
            message,
            location: None,
        })
        .collect();
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

    #[test]
    fn source_snapshot_location_is_rejected_by_rendering() {
        let input = r#"[
            {"severity":"warning","message":"missing summary"},
            {"severity":"error","message":"missing title","related":[
                {"message":"declared here","location":{"source":"guide"}}
            ]}
        ]"#;
        assert!(matches!(
            parse_diagnostics(input),
            Err(RenderInputError::MissingPath { number: 2, field })
                if field == "related[0].location"
        ));
    }
}
