//! An optional result file owned by one host invocation, separate from process logs.

use std::path::{Path, PathBuf};

use super::{DEFAULT_MAX_REPORT_BYTES, ReportBatch, ReportError, ReportSources};
use tola_diagnostic::Diagnostic;

/// Set only for a managed invocation that consumes the optional report after completion.
pub const REPORT_PATH_ENV: &str = "TOLA_REPORT_PATH";

/// Own the report directory until the command exits and its complete batch is read.
pub struct ReportChannel {
    _directory: tempfile::TempDir,
    path: PathBuf,
    producer: String,
    sources: ReportSources,
}

impl ReportChannel {
    /// Create the report directory under the process's own temporary directory.
    pub fn new(producer: impl Into<String>, sources: ReportSources) -> Result<Self, ReportError> {
        Self::new_in(&std::env::temp_dir(), producer, sources)
    }

    /// Create the report directory under the temporary root its caller already owns.
    pub fn new_in(
        base: &Path,
        producer: impl Into<String>,
        sources: ReportSources,
    ) -> Result<Self, ReportError> {
        let producer = producer.into();
        if producer.trim().is_empty() {
            return Err(ReportError::MissingProducer);
        }
        let directory = tempfile::Builder::new()
            .prefix("tola-report-")
            .tempdir_in(base)?;
        let path = std::path::absolute(directory.path())?.join("report.json");
        Ok(Self {
            _directory: directory,
            path,
            producer,
            sources,
        })
    }

    /// The child writes a complete schema-v1 report here, preferably by atomic replacement.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consume one completed invocation's report.
    pub fn read(self) -> Result<Option<Vec<Diagnostic>>, ReportError> {
        ReportBatch::read_optional(&self.path, DEFAULT_MAX_REPORT_BYTES)?
            .map(|batch| batch.validate(&self.producer, &self.sources))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_read_states() {
        let absent = ReportChannel::new("command:catalog.scan", ReportSources::new()).unwrap();
        assert!(absent.path().is_absolute());
        let directory = absent.path().parent().unwrap().to_owned();
        assert!(absent.read().unwrap().is_none());
        assert!(!directory.exists());

        let empty = ReportChannel::new("command:catalog.scan", ReportSources::new()).unwrap();
        std::fs::write(empty.path(), br#"{"schemaVersion":1,"diagnostics":[]}"#).unwrap();
        assert!(empty.read().unwrap().unwrap().is_empty());

        let invalid = ReportChannel::new("command:catalog.scan", ReportSources::new()).unwrap();
        std::fs::write(invalid.path(), b"").unwrap();
        assert!(matches!(invalid.read(), Err(ReportError::Json(_))));
    }
}
