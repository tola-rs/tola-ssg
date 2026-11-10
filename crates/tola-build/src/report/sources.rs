//! Host-registered text and its projection into diagnostic locations.

use std::collections::{BTreeMap, btree_map::Entry};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use tola_diagnostic::{DiagnosticSource, Location, SourceLine, SourcePosition, SourceRange};
use tola_typst::{ContentDigest, ReadLocator};
use tola_typst_syntax::position::{byte_offset, checked_position};
use typst::syntax::{FileId, Lines};

use super::{ReportError, ReportLocation};

pub(super) const EXCERPT_BYTES: usize = 4096;
const EXCERPT_LINES: usize = 3;

/// Source evidence registered by the host from the inputs actually supplied to the producer.
#[derive(Debug, Clone)]
pub struct ReportSource {
    evidence: DiagnosticSource,
    native_id: Option<FileId>,
    display_path: Option<String>,
    text: Arc<str>,
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
            evidence: DiagnosticSource::new(identity, digest),
            native_id,
            display_path,
            text,
        })
    }

    /// Supply an actual host-resolved editor file. The display label is never used as a URI.
    pub fn with_editor_path(mut self, path: PathBuf) -> Result<Self, ReportError> {
        if !path.is_absolute() {
            return Err(ReportError::InvalidSource(
                "an editor path must be absolute".into(),
            ));
        }
        self.evidence = self.evidence.with_editor_path(path);
        Ok(self)
    }

    pub fn evidence(&self) -> DiagnosticSource {
        self.evidence.clone()
    }

    pub fn identity(&self) -> &ReadLocator {
        self.evidence.input_identity().0
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
        self.evidence.input_identity().1
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
        match self.sources.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(source);
                Ok(())
            }
            Entry::Occupied(entry) => Err(ReportError::DuplicateSource(entry.key().clone())),
        }
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
    pub(super) fn location(
        &self,
        location: &ReportLocation,
    ) -> Result<(Option<Location>, DiagnosticSource), ReportError> {
        let source = self
            .get(&location.source)
            .ok_or_else(|| ReportError::UnknownSource(location.source.clone()))?;
        if let Some(digest) = &location.digest
            && digest != &source.digest().to_hex()
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

/// A bounded host-generated window. UTF-16 ranges remain complete even when excerpts truncate.
fn excerpt(lines: &Lines<&str>, bytes: Range<usize>, range: SourceRange) -> Vec<SourceLine> {
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
        let start = text.ceil_char_boundary(highlight_start.saturating_sub(remaining / 4));
        let end = text.floor_char_boundary(start.saturating_add(remaining));
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
