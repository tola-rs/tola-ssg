//! Versioned reports from an already running external program.

mod channel;
mod sources;

pub use channel::{REPORT_PATH_ENV, ReportChannel};
pub use sources::{ReportSource, ReportSources};

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use tola_diagnostic::{
    Diagnostic, DiagnosticProducer, Help, RelatedInformation, Severity, SourcePosition, SourceRange,
};

/// The only report schema this implementation accepts.
pub const REPORT_SCHEMA_VERSION: u32 = 1;
/// A default bound for one complete JSON report, independently of captured process logs.
pub const DEFAULT_MAX_REPORT_BYTES: usize = 8 * 1024 * 1024;

/// One complete report. A successful empty batch is distinct from a missing report file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportBatch {
    pub schema_version: u32,
    pub diagnostics: Vec<ReportDiagnostic>,
}

/// Public issue data. Ownership and core diagnostic causes cannot be supplied on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportDiagnostic {
    pub severity: Severity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<ReportLocation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<ReportRelated>,
}

/// A range into a host-registered immutable source, optionally confirming its BLAKE3 digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportLocation {
    pub source: String,
    #[serde(deserialize_with = "deserialize_range")]
    pub range: SourceRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// Context accompanying an issue; an absent location remains absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRelated {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<ReportLocation>,
}

fn deserialize_range<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SourceRange, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Position {
        line: usize,
        character: usize,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Range {
        start: Position,
        end: Position,
    }
    let range = Range::deserialize(deserializer)?;
    Ok(SourceRange {
        start: SourcePosition {
            line: range.start.line,
            character: range.start.character,
        },
        end: SourcePosition {
            line: range.end.line,
            character: range.end.character,
        },
    })
}

/// A report is absent, malformed, too large, or invalid; never an empty success.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("could not read diagnostic report: {0}")]
    Io(#[from] io::Error),
    #[error("diagnostic report exceeds the {limit}-byte limit")]
    TooLarge { limit: usize },
    #[error("invalid diagnostic report JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported diagnostic report schema version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid host report source: {0}")]
    InvalidSource(String),
    #[error("report source ID `{0}` is already registered")]
    DuplicateSource(String),
    #[error("diagnostic report names unknown source ID `{0}`")]
    UnknownSource(String),
    #[error("invalid diagnostic location for source `{source_id}`: {reason}")]
    InvalidLocation { source_id: String, reason: String },
    #[error("invalid diagnostic report item {index}: {reason}")]
    InvalidDiagnostic { index: usize, reason: String },
    #[error("a diagnostic report requires a nonempty host producer identity")]
    MissingProducer,
}

impl ReportBatch {
    pub fn empty() -> Self {
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            diagnostics: Vec::new(),
        }
    }

    /// Read one bounded JSON value, refusing trailing data and unsupported schemas.
    pub fn from_reader(reader: impl Read, max_bytes: usize) -> Result<Self, ReportError> {
        let cap = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::new();
        reader.take(cap).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(ReportError::TooLarge { limit: max_bytes });
        }
        let batch: Self = serde_json::from_slice(&bytes)?;
        batch.check_version()?;
        Ok(batch)
    }

    /// Missing optional files return `None`.
    pub fn read_optional(path: &Path, max_bytes: usize) -> Result<Option<Self>, ReportError> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Self::from_reader(file, max_bytes).map(Some)
    }

    fn check_version(&self) -> Result<(), ReportError> {
        if self.schema_version != REPORT_SCHEMA_VERSION {
            return Err(ReportError::UnsupportedVersion(self.schema_version));
        }
        Ok(())
    }

    /// Validate a complete batch atomically against the host's actual input snapshot.
    pub fn validate(
        self,
        producer: &str,
        sources: &ReportSources,
    ) -> Result<Vec<Diagnostic>, ReportError> {
        self.check_version()?;
        if producer.trim().is_empty() {
            return Err(ReportError::MissingProducer);
        }
        let mut validated = Vec::with_capacity(self.diagnostics.len());
        for (index, item) in self.diagnostics.into_iter().enumerate() {
            let invalid = |reason: &str| ReportError::InvalidDiagnostic {
                index,
                reason: reason.into(),
            };
            if item.message.trim().is_empty() {
                return Err(invalid("message must not be empty"));
            }
            if item
                .code
                .as_ref()
                .is_some_and(|code| code.trim().is_empty())
            {
                return Err(invalid("code must not be empty"));
            }
            if item.notes.iter().any(|note| note.trim().is_empty()) {
                return Err(invalid("notes must not be empty"));
            }
            if item.hints.iter().any(|hint| hint.trim().is_empty()) {
                return Err(invalid("hints must not be empty"));
            }
            let mut diagnostic = Diagnostic::new(
                tola_diagnostic::codes::report::ISSUE,
                item.severity,
                item.message,
            );
            diagnostic.producer = Some(DiagnosticProducer {
                name: Some(producer.to_owned()),
                code: item.code,
            });
            if let Some(location) = &item.location {
                let (place, evidence) = sources.location(location)?;
                diagnostic.location = place;
                diagnostic.source = Some(evidence);
            }
            diagnostic.help = item
                .hints
                .into_iter()
                .map(|message| Help {
                    message,
                    location: None,
                })
                .collect();
            diagnostic.notes = item.notes;
            for related in item.related {
                if related.message.trim().is_empty() {
                    return Err(invalid("related messages must not be empty"));
                }
                let (place, evidence) = match &related.location {
                    Some(location) => {
                        let (place, evidence) = sources.location(location)?;
                        (place, Some(evidence))
                    }
                    None => (None, None),
                };
                diagnostic.related.push(RelatedInformation {
                    message: related.message,
                    location: place,
                    source: evidence,
                });
            }
            validated.push(diagnostic);
        }
        tola_diagnostic::coalesce_reports(&mut validated);
        Ok(validated)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use tola_typst::ReadLocator;

    use super::sources::EXCERPT_BYTES;

    use super::*;

    fn parse(value: Value) -> Result<ReportBatch, ReportError> {
        ReportBatch::from_reader(value.to_string().as_bytes(), DEFAULT_MAX_REPORT_BYTES)
    }

    fn batch(items: Value) -> ReportBatch {
        parse(json!({"schemaVersion": 1, "diagnostics": items})).unwrap()
    }

    fn position(line: usize, character: usize) -> SourcePosition {
        SourcePosition { line, character }
    }

    fn range(start: usize, end: usize) -> SourceRange {
        SourceRange {
            start: position(0, start),
            end: position(0, end),
        }
    }

    fn source(text: &str, path: Option<&str>) -> ReportSource {
        ReportSource::new(
            ReadLocator::ProvidedRoot("article.typ".into()),
            Some(tola_typst::file_id("article.typ")),
            path.map(str::to_owned),
            Arc::from(text),
            None,
        )
        .unwrap()
    }

    fn sources(text: &str) -> ReportSources {
        let mut sources = ReportSources::new();
        sources
            .register("source-1", source(text, Some("article.typ")))
            .unwrap();
        sources
    }

    fn located_issue(range: SourceRange) -> Value {
        json!({
            "severity": "warning", "message": "Review this value.", "code": "dynamic-rule/中文",
            "location": {"source": "source-1", "range": range}
        })
    }

    #[test]
    fn report_fields() {
        let report = batch(json!([
            {"severity": "error", "message": "unknown variable", "code": "typst.compile",
             "notes": ["first", "second"], "hints": ["Correct the value."], "related": [{"message": "Context without a position."}]},
            {"severity": "warning", "message": "Warning"},
            {"severity": "info", "message": "Information"},
            {"severity": "hint", "message": "Advice"}
        ]));
        let encoded = serde_json::to_vec(&report).unwrap();
        let report = ReportBatch::from_reader(encoded.as_slice(), encoded.len()).unwrap();
        let diagnostics = report
            .validate("command:catalog", &ReportSources::new())
            .unwrap();
        assert_eq!(
            diagnostics.iter().map(|d| d.severity).collect::<Vec<_>>(),
            [
                Severity::Error,
                Severity::Warning,
                Severity::Info,
                Severity::Hint
            ]
        );
        assert_eq!(
            diagnostics[0].producer.as_ref().unwrap().name.as_deref(),
            Some("command:catalog")
        );
        assert_eq!(
            diagnostics[0].producer.as_ref().unwrap().code.as_deref(),
            Some("typst.compile")
        );
        assert_eq!(diagnostics[0].code, tola_diagnostic::codes::report::ISSUE);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.location.is_none() && d.cause.is_none())
        );
        assert_eq!(diagnostics[0].notes, ["first", "second"]);
        assert_eq!(diagnostics[0].help[0].message, "Correct the value.");
        assert!(diagnostics[0].related[0].location.is_none());
        assert_eq!(diagnostics[0].display_message(), "unknown variable");
        assert!(
            ReportBatch::empty()
                .validate("command:catalog", &ReportSources::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn utf16_locations() {
        let sources = sources("a🦊b\r\n中文\n");
        let source = sources.get("source-1").unwrap();
        assert_eq!(source.native_id(), Some(tola_typst::file_id("article.typ")));
        assert!(matches!(source.identity(), ReadLocator::ProvidedRoot(_)));
        let mut issue = located_issue(range(1, 3));
        issue["location"]["digest"] = json!(source.digest().to_hex());
        issue["related"] = json!([{"message":"Other line", "location": {
            "source":"source-1", "range": {"start":{"line":1,"character":0},"end":{"line":1,"character":2}}
        }}]);
        let diagnostic = batch(json!([issue]))
            .validate("checker:sample", &sources)
            .unwrap()
            .remove(0);
        assert_eq!(
            diagnostic.source.as_ref().unwrap().input_identity(),
            (source.identity(), source.digest())
        );
        assert!(diagnostic.source.as_ref().unwrap().editor_path().is_none());
        assert_eq!(
            diagnostic.related[0]
                .source
                .as_ref()
                .unwrap()
                .input_identity(),
            (source.identity(), source.digest())
        );
        let at = diagnostic.location.unwrap();
        assert_eq!(at.path, "article.typ");
        assert_eq!(at.line, Some(1));
        assert_eq!(at.column, Some(2));
        assert_eq!(at.range, Some(range(1, 3)));
        assert_eq!(at.source_lines[0].text, "a🦊b");
        assert_eq!(at.source_lines[0].highlight, Some((1, 5)));
        assert_eq!(
            diagnostic.related[0].location.as_ref().unwrap().line,
            Some(2)
        );
        assert_eq!(
            diagnostic.related[0]
                .location
                .as_ref()
                .unwrap()
                .source_lines[0]
                .text,
            "中文"
        );
    }

    #[test]
    fn invalid_locations() {
        let sources = sources("a🦊b\n");
        for invalid in [
            json!({"source":"/a/real/or/guessed/path.typ", "range": range(0,1)}),
            json!({"source":"source-1", "range": range(2,3)}),
            json!({"source":"source-1", "range": range(3,1)}),
            json!({"source":"source-1", "range": range(0,5)}),
            json!({"source":"source-1", "range":{"start":{"line":2,"character":0},"end":{"line":2,"character":0}}}),
            json!({"source":"source-1", "range": range(0,1), "digest":"0".repeat(64)}),
            json!({"source":"source-1", "range":{"start":{"line":0,"character":u64::MAX},"end":{"line":0,"character":u64::MAX}}}),
        ] {
            let mut issue = located_issue(range(0, 1));
            issue["location"] = invalid;
            assert!(
                parse(json!({"schemaVersion":1,"diagnostics":[issue]}))
                    .and_then(|batch| batch.validate("checker:sample", &sources))
                    .is_err()
            );
        }
    }

    #[test]
    fn source_snapshot() {
        let old = source("old source", Some("article.typ"));
        let old_digest = old.digest();
        let mut sources = ReportSources::new();
        sources.register("source-1", old.clone()).unwrap();
        assert!(matches!(
            sources.register("source-1", source("new source", Some("article.typ"))),
            Err(ReportError::DuplicateSource(_))
        ));
        assert!(sources.register("", old).is_err());
        assert_eq!(sources.get("source-1").unwrap().text(), "old source");
        assert!(
            ReportSource::new(
                ReadLocator::ProvidedRoot("article.typ".into()),
                None,
                Some("article.typ".into()),
                Arc::from("new source"),
                Some(old_digest)
            )
            .is_err()
        );
        let mut changed = ReportSources::new();
        changed
            .register("source-1", source("new source", Some("article.typ")))
            .unwrap();
        let mut issue = located_issue(range(0, 3));
        issue["location"]["digest"] = json!(old_digest.to_hex());
        let old_batch = batch(json!([issue]));
        assert!(
            old_batch
                .clone()
                .validate("checker:sample", &sources)
                .is_ok()
        );
        assert!(old_batch.validate("checker:sample", &changed).is_err());
    }

    #[test]
    fn unlabeled_source() {
        let mut sources = ReportSources::new();
        sources
            .register("source-1", source("sample", None))
            .unwrap();
        let diagnostics = batch(json!([located_issue(range(0, 3))]))
            .validate("checker:sample", &sources)
            .unwrap();
        assert!(diagnostics[0].location.is_none());
        assert!(diagnostics[0].source.is_some());
        assert!(
            batch(json!([located_issue(range(0, 99))]))
                .validate("checker:sample", &sources)
                .is_err()
        );
    }

    #[test]
    fn editor_path() {
        let directory = tempfile::tempdir().unwrap();
        let actual = directory.path().join("actual.typ");
        let mapped = source("sample", Some("a display label"))
            .with_editor_path(actual.clone())
            .unwrap();
        assert!(
            source("sample", Some("article.typ"))
                .evidence()
                .editor_path()
                .is_none()
        );
        assert!(
            source("sample", Some("article.typ"))
                .with_editor_path("relative.typ".into())
                .is_err()
        );
        let mut sources = ReportSources::new();
        sources.register("source-1", mapped).unwrap();
        let diagnostic = batch(json!([located_issue(range(0, 3))]))
            .validate("checker:sample", &sources)
            .unwrap()
            .remove(0);
        assert_eq!(
            diagnostic.location.as_ref().unwrap().path,
            "a display label"
        );
        assert_eq!(
            diagnostic.source.as_ref().unwrap().editor_path(),
            Some(actual.as_path())
        );
        let serialized = serde_json::to_value(&diagnostic).unwrap();
        assert!(
            serialized.get("source").is_none(),
            "trusted input identity stays internal"
        );
    }

    #[test]
    fn rejects_forged_fields() {
        let base =
            json!({"schemaVersion":1,"diagnostics":[{"severity":"warning","message":"sample"}]});
        for field in ["producer", "snapshot", "owner", "schema_version"] {
            let mut value = base.clone();
            value[field] = json!("forged");
            assert!(parse(value).is_err(), "accepted batch {field}");
        }
        for field in [
            "producer",
            "owner",
            "cause",
            "source_lines",
            "fix",
            "callback",
            "editorPath",
            "editor_path",
        ] {
            let mut value = base.clone();
            value["diagnostics"][0][field] = json!("forged");
            assert!(parse(value).is_err(), "accepted issue {field}");
        }
        for field in [
            "path",
            "file",
            "source_lines",
            "line",
            "owner",
            "editorPath",
            "editor_path",
        ] {
            let mut issue = located_issue(range(0, 1));
            issue["location"][field] = json!("forged");
            assert!(parse(json!({"schemaVersion":1,"diagnostics":[issue]})).is_err());
        }
        let mut issue = located_issue(range(0, 1));
        issue["location"]["range"]["start"]["owner"] = json!("forged");
        assert!(parse(json!({"schemaVersion":1,"diagnostics":[issue]})).is_err());
        assert!(
            parse(json!({"schemaVersion":1,"diagnostics":[{"severity":"fatal","message":"x"}]}))
                .is_err()
        );
        assert!(matches!(
            parse(json!({"schemaVersion":2,"diagnostics":[]})),
            Err(ReportError::UnsupportedVersion(2))
        ));
        assert!(parse(json!({"schemaVersion":1})).is_err());
    }

    #[test]
    fn rejects_invalid_batch() {
        let report = batch(json!([
            {"severity":"warning","message":"valid"},
            located_issue(range(0,1))
        ]));
        assert!(matches!(
            report.validate("command:sample", &ReportSources::new()),
            Err(ReportError::UnknownSource(_))
        ));
        assert!(matches!(
            ReportBatch::empty().validate("", &ReportSources::new()),
            Err(ReportError::MissingProducer)
        ));
        for issue in [
            json!({"severity":"warning","message":" "}),
            json!({"severity":"warning","message":"x","code":" "}),
            json!({"severity":"warning","message":"x","notes":[""]}),
            json!({"severity":"warning","message":"x","hints":[""]}),
            json!({"severity":"warning","message":"x","related":[{"message":""}]}),
        ] {
            assert!(
                batch(json!([issue]))
                    .validate("command:sample", &ReportSources::new())
                    .is_err()
            );
        }
    }

    #[test]
    fn merges_by_input() {
        let mut sources = sources("sample");
        sources
            .register(
                "source-2",
                ReportSource::new(
                    ReadLocator::ProvidedRoot("different-input.typ".into()),
                    Some(tola_typst::file_id("different-input.typ")),
                    Some("article.typ".into()),
                    Arc::from("sample"),
                    None,
                )
                .unwrap(),
            )
            .unwrap();
        let mut first = located_issue(range(0, 3));
        first["hints"] = json!(["First correction"]);
        first["related"] = json!([{"message":"First context"}]);
        let mut second = first.clone();
        second["hints"] = json!(["Second correction"]);
        second["related"] = json!([{"message":"Second context"}]);
        let merged = batch(json!([first.clone(), second.clone()]))
            .validate("checker:sample", &sources)
            .unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].help.len(), 2);
        assert_eq!(merged[0].related.len(), 2);
        second["location"]["source"] = json!("source-2");
        assert_eq!(
            batch(json!([first, second]))
                .validate("checker:sample", &sources)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            batch(json!([
                {"severity":"warning","message":"unlocated"},
                {"severity":"warning","message":"unlocated"}
            ]))
            .validate("checker:sample", &sources)
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn bounded_excerpt() {
        let text = format!("{}🦊{}", "x".repeat(8000), "z".repeat(8000));
        let sources = sources(&text);
        let diagnostic = batch(json!([located_issue(range(8000, 8002))]))
            .validate("checker:sample", &sources)
            .unwrap()
            .remove(0);
        let at = diagnostic.location.unwrap();
        assert_eq!(at.range, Some(range(8000, 8002)));
        assert!(
            at.source_lines
                .iter()
                .map(|line| line.text.len())
                .sum::<usize>()
                <= EXCERPT_BYTES
        );
        let excerpt = &at.source_lines[0];
        assert!(excerpt.start_character > 0);
        assert!(!excerpt.ends_line);
        let (start, end) = excerpt.highlight.unwrap();
        assert_eq!(&excerpt.text[start..end], "🦊");
    }

    #[test]
    fn report_file_states() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        assert!(ReportBatch::read_optional(&path, 1024).unwrap().is_none());
        std::fs::write(&path, b"").unwrap();
        assert!(matches!(
            ReportBatch::read_optional(&path, 1024),
            Err(ReportError::Json(_))
        ));
        std::fs::write(&path, br#"{"schemaVersion":1,"diagnostics":[]}"#).unwrap();
        assert!(
            ReportBatch::read_optional(&path, 1024)
                .unwrap()
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert!(matches!(
            ReportBatch::read_optional(&path, 8),
            Err(ReportError::TooLarge { limit: 8 })
        ));
        std::fs::write(&path, br#"{"schemaVersion":1,"diagnostics":[]}"#).unwrap();
        let mut trailing = std::fs::read(&path).unwrap();
        trailing.extend_from_slice(b" false");
        assert!(matches!(
            ReportBatch::from_reader(trailing.as_slice(), 1024),
            Err(ReportError::Json(_))
        ));
    }

    #[test]
    fn report_io_error() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            ReportBatch::read_optional(directory.path(), 1024),
            Err(ReportError::Io(_))
        ));
    }
}
