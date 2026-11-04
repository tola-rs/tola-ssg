//! Render diagnostic JSON through the renderer builds use.
//!
//! The input is the shape scripts write with diagnostic issue: a list of issue records, or a
//! report object with a diagnostics array. This command does not define a second layout; it
//! converts the records to the same Diagnostic the build renders and prints the result.

use std::io::Read;

use anyhow::{Context, Result};
use serde::Deserialize;
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
    path: String,
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
    List(Vec<Issue>),
    Report { diagnostics: Vec<Issue> },
}

pub(in crate::cli) fn render(output: &CommandOutput) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("read diagnostic JSON from stdin")?;
    let issues = match serde_json::from_str::<Input>(&input).context("parse diagnostic JSON")? {
        Input::List(issues)
        | Input::Report {
            diagnostics: issues,
        } => issues,
    };
    if issues.is_empty() {
        return Ok(());
    }
    let sources = crate::terminal::SourceFiles::new(
        std::env::current_dir()?,
        tola_typst::PackageLocations::discover(None, None)?,
    );
    for issue in issues {
        let diagnostic = into_diagnostic(issue)?;
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

fn into_diagnostic(issue: Issue) -> Result<Diagnostic> {
    let severity = match issue.severity.as_str() {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        "info" => Severity::Info,
        "hint" => Severity::Hint,
        other => anyhow::bail!("unknown diagnostic severity {other}"),
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
        .map(|related| RelatedInformation {
            message: related.message,
            location: related.location.map(into_location),
            source: None,
        })
        .collect();
    diagnostic.location = issue.location.map(into_location);
    Ok(diagnostic)
}

fn into_location(location: IssueLocation) -> Location {
    Location {
        path: location.path,
        line: location.line,
        column: location.column,
        range: location.range,
        source_lines: Vec::new(),
    }
}
