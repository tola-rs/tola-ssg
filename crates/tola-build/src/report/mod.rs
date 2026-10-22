//! Versioned reports from an already running external program.

mod channel;

pub use channel::{REPORT_PATH_ENV, ReportChannel};

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use tola_typst::{ContentDigest, ReadLocator};
use tola_typst_syntax::position::{byte_offset, checked_position};
use typst::syntax::{FileId, Lines};

use tola_diagnostic::{
    Diagnostic, DiagnosticProducer, DiagnosticSource, Help, Location, RelatedInformation, Severity,
    SourceLine, SourcePosition, SourceRange,
};

/// The only report schema this implementation accepts.
pub const REPORT_SCHEMA_VERSION: u32 = 1;
/// A default bound for one complete JSON report, independently of captured process logs.
pub const DEFAULT_MAX_REPORT_BYTES: usize = 8 * 1024 * 1024;
const EXCERPT_BYTES: usize = 4096;
const EXCERPT_LINES: usize = 3;

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
    #[serde(deserialize_with = "deserialize_severity")]
    pub severity: Severity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<ReportLocation>,
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

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

fn deserialize_severity<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Severity, D::Error> {
    Ok(match WireSeverity::deserialize(deserializer)? {
        WireSeverity::Error => Severity::Error,
        WireSeverity::Warning => Severity::Warning,
        WireSeverity::Info => Severity::Info,
        WireSeverity::Hint => Severity::Hint,
    })
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

/// Source evidence registered by the host from the inputs actually supplied to the producer.
#[derive(Debug, Clone)]
pub struct ReportSource {
    identity: ReadLocator,
    native_id: Option<FileId>,
    display_path: Option<String>,
    text: Arc<str>,
    digest: ContentDigest,
    editor_path: Option<PathBuf>,
}

impl ReportSource {
    /// Bind the exact text and its identity; a known read digest must agree with that text.
    pub fn new(
        identity: ReadLocator,
        native_id: Option<FileId>,
        display_path: Option<String>,
        text: Arc<str>,
        expected_digest: Option<ContentDigest>,
    ) -> Result<Self, ReportError> {
        if display_path.as_ref().is_some_and(|path| path.is_empty()) {
            return Err(ReportError::InvalidSource(
                "a display path must not be empty".into(),
            ));
        }
        let digest = ContentDigest::of(text.as_bytes());
        if expected_digest.is_some_and(|expected| expected != digest) {
            return Err(ReportError::InvalidSource(
                "source text disagrees with its read digest".into(),
            ));
        }
        Ok(Self {
            identity,
            native_id,
            display_path,
            text,
            digest,
            editor_path: None,
        })
    }

    /// Supply an actual host-resolved editor file. The display label is never used as a URI.
    pub fn with_editor_path(mut self, path: PathBuf) -> Result<Self, ReportError> {
        if !path.is_absolute() {
            return Err(ReportError::InvalidSource(
                "an editor path must be absolute".into(),
            ));
        }
        self.editor_path = Some(path);
        Ok(self)
    }

    pub fn evidence(&self) -> DiagnosticSource {
        let source = DiagnosticSource::new(self.identity.clone(), self.digest);
        match &self.editor_path {
            Some(path) => source.with_editor_path(path.clone()),
            None => source,
        }
    }

    pub fn identity(&self) -> &ReadLocator {
        &self.identity
    }
    pub fn native_id(&self) -> Option<FileId> {
        self.native_id
    }
    pub fn display_path(&self) -> Option<&str> {
        self.display_path.as_deref()
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }
}

/// An immutable-text source registry for one host input snapshot.
#[derive(Debug, Clone, Default)]
pub struct ReportSources {
    sources: BTreeMap<String, ReportSource>,
}

impl ReportSources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        id: impl Into<String>,
        source: ReportSource,
    ) -> Result<(), ReportError> {
        let id = id.into();
        if id.is_empty() {
            return Err(ReportError::InvalidSource(
                "a source ID must not be empty".into(),
            ));
        }
        if self.sources.contains_key(&id) {
            return Err(ReportError::DuplicateSource(id));
        }
        self.sources.insert(id, source);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ReportSource> {
        self.sources.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ReportSource)> {
        self.sources
            .iter()
            .map(|(id, source)| (id.as_str(), source))
    }

    /// The display location and evidence one registered source supplies.
    fn location(
        &self,
        location: &ReportLocation,
    ) -> Result<(Option<Location>, DiagnosticSource), ReportError> {
        let source = self
            .get(&location.source)
            .ok_or_else(|| ReportError::UnknownSource(location.source.clone()))?;
        if let Some(digest) = &location.digest
            && digest != &source.digest.to_hex()
        {
            return Err(ReportError::InvalidLocation {
                source_id: location.source.clone(),
                reason: "the reported digest does not match the supplied source snapshot".into(),
            });
        }
        let lines = Lines::new(source.text());
        let offset = |position: SourcePosition| {
            let position = checked_position(position.line, position.character)
                .ok_or_else(|| "source coordinates exceed the UTF-16 protocol limit".to_owned())?;
            byte_offset(&lines, position).map_err(|error| error.to_string())
        };
        let invalid_location = |reason: String| ReportError::InvalidLocation {
            source_id: location.source.clone(),
            reason,
        };
        let start = offset(location.range.start).map_err(invalid_location)?;
        let end = offset(location.range.end).map_err(invalid_location)?;
        if start > end {
            return Err(invalid_location("the source range is reversed".into()));
        }
        let Some(path) = source.display_path() else {
            return Ok((None, source.evidence()));
        };
        let line_start = lines
            .line_to_byte(location.range.start.line)
            .expect("a validated source line");
        let column = source.text()[line_start..start].chars().count() + 1;
        Ok((
            Some(Location {
                path: path.to_owned(),
                line: Some(location.range.start.line + 1),
                column: Some(column),
                range: Some(location.range),
                source_lines: excerpt(&lines, start..end, location.range),
            }),
            source.evidence(),
        ))
    }
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
        let mut validated = Vec::new();
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

/// A bounded host-generated window. UTF-16 ranges remain complete even when excerpts truncate.
fn excerpt(
    lines: &Lines<&str>,
    bytes: std::ops::Range<usize>,
    range: SourceRange,
) -> Vec<SourceLine> {
    let last = range
        .end
        .line
        .min(range.start.line.saturating_add(EXCERPT_LINES - 1));
    let mut result = Vec::new();
    let mut remaining = EXCERPT_BYTES;
    for line in range.start.line..=last {
        let line_range = lines.line_to_range(line).expect("a validated source line");
        let text = lines.text()[line_range.clone()].trim_end_matches(typst::syntax::is_newline);
        let highlight_start = bytes.start.saturating_sub(line_range.start).min(text.len());
        let highlight_end = bytes.end.saturating_sub(line_range.start).min(text.len());
        let mut start = highlight_start.saturating_sub(remaining / 4);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let mut end = text.len().min(start.saturating_add(remaining));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let window = &text[start..end];
        let highlight = (highlight_start.max(start) < highlight_end.min(end)).then_some((
            highlight_start.max(start) - start,
            highlight_end.min(end) - start,
        ));
        result.push(SourceLine {
            line: line + 1,
            start_column: text[..start].chars().count(),
            start_character: text[..start].encode_utf16().count(),
            ends_line: end == text.len(),
            text: window.to_owned(),
            highlight,
        });
        remaining -= window.len();
        if remaining == 0 {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

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
    fn wire_fields_become_public_reports() {
        let report = batch(json!([
            {"severity": "error", "message": "unknown variable", "code": "typst.compile",
             "hints": ["Correct the value."], "related": [{"message": "Context without a position."}]},
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
    fn registered_sources_produce_utf16_excerpts() {
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
    fn invalid_source_places_are_rejected() {
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
    fn source_registration_keeps_text_immutable() {
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
    fn unlabeled_sources_stay_unlocated() {
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
    fn editor_paths_need_host_file() {
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
    fn wire_refuses_forged_fields() {
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
    fn batch_validation_is_atomic() {
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
    fn contexts_merge_for_one_input() {
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
    fn excerpts_stay_bounded_and_exact() {
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
    fn optional_report_read_states() {
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
    fn read_failure_is_distinct() {
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unreadable report",
                ))
            }
        }
        assert!(matches!(
            ReportBatch::from_reader(FailedReader, 1024),
            Err(ReportError::Io(_))
        ));
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            ReportBatch::read_optional(directory.path(), 1024),
            Err(ReportError::Io(_))
        ));
    }
}
