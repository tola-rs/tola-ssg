//! Deferred, bounded startup recording and append-only command logs.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};

use crate::terminal::display_path;

pub(crate) mod destination;
pub(crate) mod tracing;

const MAX_BUFFERED_STARTUP_BYTES: usize = 256 * 1024;
const MAX_LOG_HEADER_BYTES: u64 = 8 * 1024;

/// How many paths one debug record names before its count stands for the rest.
///
/// A record that listed every path of a large batch would bury the event it describes; the count
/// beside the sample says what was left out.
pub(crate) const LOGGED_PATH_SAMPLE: usize = 8;

/// The current UTC time, as the record envelope and a session file name both read it.
fn utc_stamp() -> String {
    let mut formatted = String::new();
    SystemTime
        .format_time(&mut Writer::new(&mut formatted))
        .expect("formatting into a String cannot fail");
    formatted
}

/// A filesystem-safe UTC stamp for one session log file.
pub(crate) fn session_stamp() -> String {
    utc_stamp()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

/// Why a command records a log file.
///
/// The origin decides who answers for a log this session cannot use: Tola names the development
/// session log itself, so an unusable file must not stop the session, while a file the reader
/// named with `--log-file` must fail loudly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogOrigin {
    /// The development session log Tola names below the site's reserved directory.
    Automatic,
    /// The file the reader named with `--log-file`.
    Explicit,
}

#[derive(Clone)]
pub(crate) struct LogFile {
    inner: Arc<LogFileInner>,
}

type WriteFailure = dyn Fn(&Path, &io::Error) + Send + Sync;

struct LogFileInner {
    path: PathBuf,
    /// The path as declared, before a symlinked prefix resolves to its target: diagnostics for a
    /// log Tola named itself report this one.
    declared_path: PathBuf,
    origin: LogOrigin,
    destination: Mutex<LogDestination>,
    on_write_failure: Box<WriteFailure>,
}

enum LogDestination {
    /// Records wait here until the session knows the paths its command writes.
    Pending(StartupRecords),
    /// The command's log, open for appending; `stopped` marks a failed write, after which records
    /// are dropped. The handle stays open so every later record appends to the file this command
    /// opened.
    Recording { file: File, stopped: bool },
    /// Preparing or opening the log failed, or the caller refused it: this session records
    /// nothing.
    Unavailable,
}

#[derive(Default)]
struct StartupRecords {
    records: VecDeque<Vec<u8>>,
    retained_bytes: usize,
    omitted_records: usize,
}

impl StartupRecords {
    fn push(&mut self, record: &[u8]) {
        if record.len() > MAX_BUFFERED_STARTUP_BYTES {
            self.omitted_records += 1;
            return;
        }
        while self.retained_bytes + record.len() > MAX_BUFFERED_STARTUP_BYTES {
            // Keep the session header; evict the oldest subsequent record.
            let Some(evicted) = self.records.remove(1) else {
                self.omitted_records += 1;
                return;
            };
            self.retained_bytes -= evicted.len();
            self.omitted_records += 1;
        }
        self.records.push_back(record.to_vec());
        self.retained_bytes += record.len();
    }
}

impl LogFile {
    /// Check a log path the reader named with `--log-file` without creating files or directories.
    ///
    /// The callback reports the first runtime write failure after releasing the
    /// destination lock. It must not emit tracing into this recorder.
    pub(crate) fn prepare(
        path: &Path,
        on_write_failure: impl Fn(&Path, &io::Error) + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::prepare_with(path, LogOrigin::Explicit, on_write_failure)
    }

    /// Check the development session log Tola names below the site's reserved directory.
    ///
    /// The session owns this file rather than the reader: a file Tola cannot use is reported as a
    /// warning by the caller, and the session continues without it. The callback reports the first
    /// runtime write failure after releasing the destination lock and must not emit tracing into
    /// this recorder.
    pub(crate) fn prepare_session(
        path: &Path,
        on_write_failure: impl Fn(&Path, &io::Error) + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::prepare_with(path, LogOrigin::Automatic, on_write_failure)
    }

    fn prepare_with(
        path: &Path,
        origin: LogOrigin,
        on_write_failure: impl Fn(&Path, &io::Error) + Send + Sync + 'static,
    ) -> Result<Self> {
        anyhow::ensure!(
            !path.as_os_str().is_empty(),
            "`--log-file` requires a non-empty path"
        );
        require_regular_file(path)?;
        // Normalizing resolves a symlinked prefix to its target; the declared path is kept for the
        // messages about a log Tola named itself, which should read as the site-relative path.
        let declared_path = path.to_owned();
        let path = tola_build::filesystem::normalize_existing_prefix(path);
        if path.try_exists().with_context(|| {
            format!(
                "cannot use log `{}`; choose a new file",
                display_path(&path)
            )
        })? {
            let mut file = crate::sys::open_log_for_read(&path).with_context(|| {
                format!(
                    "cannot read log `{}`; choose a new file",
                    display_path(&path)
                )
            })?;
            let metadata = file.metadata().with_context(|| {
                format!(
                    "cannot use log `{}`; choose a new file",
                    display_path(&path)
                )
            })?;
            anyhow::ensure!(
                metadata.is_file(),
                "log `{}` is not a file; choose a file path for `--log-file`",
                display_path(&path)
            );
            require_single_link(&path, &file)?;
            anyhow::ensure!(
                metadata.len() != 0,
                "log `{}` is empty; choose a new file or an existing Tola command log",
                display_path(&path)
            );
            validate_contents(&path, &mut file)?;
        }
        Ok(Self {
            inner: Arc::new(LogFileInner {
                path,
                declared_path,
                origin,
                destination: Mutex::new(LogDestination::Pending(StartupRecords::default())),
                on_write_failure: Box::new(on_write_failure),
            }),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The path this log was declared with, before a symlinked prefix was resolved.
    pub(crate) fn declared_path(&self) -> &Path {
        &self.inner.declared_path
    }

    /// Why this command records a log file.
    pub(crate) fn origin(&self) -> LogOrigin {
        self.inner.origin
    }

    /// Stop this log for good: no later round opens it, and a log that already records stops
    /// writing.
    ///
    /// Reports whether this call changed the state, so the caller warns exactly once.
    pub(crate) fn disable(&self) -> bool {
        let mut destination = self
            .inner
            .destination
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*destination, LogDestination::Pending(_)) {
            *destination = LogDestination::Unavailable;
            return true;
        }
        if let LogDestination::Recording { stopped, .. } = &mut *destination
            && !*stopped
        {
            *stopped = true;
            return true;
        }
        false
    }

    /// Whether this log writes nothing more: it stopped, or it never opened.
    pub(crate) fn is_stopped(&self) -> bool {
        let destination = self
            .inner
            .destination
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*destination {
            LogDestination::Pending(_) => false,
            LogDestination::Recording { stopped, .. } => *stopped,
            LogDestination::Unavailable => true,
        }
    }

    /// Activate only after the command has checked its input and output paths.
    ///
    /// A log that cannot be opened stays unavailable: the caller reports an automatic session log
    /// as a warning, and never announces a log that did not start.
    pub(crate) fn start(&self) -> Result<bool> {
        let mut destination = self
            .inner
            .destination
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let LogDestination::Pending(startup) = &mut *destination else {
            return Ok(false);
        };
        let file = match self.open(startup) {
            Ok(file) => file,
            Err(error) => {
                *destination = LogDestination::Unavailable;
                return Err(error);
            }
        };
        *destination = LogDestination::Recording {
            file,
            stopped: false,
        };
        Ok(true)
    }

    /// Write the buffered startup records into the prepared log file.
    fn open(&self, startup: &StartupRecords) -> Result<File> {
        require_regular_file(self.path())?;
        if let Some(parent) = self.path().parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("cannot create log directory `{}`", display_path(parent))
            })?;
        }
        let mut options = OpenOptions::new();
        options.read(true).append(true);
        crate::sys::open_log_for_append(&mut options);
        let exists = self.path().try_exists().with_context(|| {
            format!(
                "cannot use log `{}`; choose a new file",
                display_path(self.path())
            )
        })?;
        if !exists {
            options.create_new(true);
        }
        let mut file = options.open(self.path()).with_context(|| {
            format!(
                "cannot open log `{}`; choose a writable file for `--log-file`",
                display_path(self.path())
            )
        })?;
        let metadata = file.metadata().with_context(|| {
            format!(
                "cannot use log `{}`; choose a new file",
                display_path(self.path())
            )
        })?;
        if !metadata.is_file() {
            bail!(
                "log `{}` is not a file; choose a file path for `--log-file`",
                display_path(self.path())
            );
        }
        require_single_link(self.path(), &file)?;
        anyhow::ensure!(
            !exists || metadata.len() != 0,
            "log `{}` is empty; choose a new file or an existing Tola command log",
            display_path(self.path())
        );
        let handle = file
            .try_clone()
            .and_then(same_file::Handle::from_file)
            .with_context(|| {
                format!(
                    "cannot use log `{}`; choose a new file",
                    display_path(self.path())
                )
            })?;
        require_path_names_file(self.path(), &file, &handle)?;
        for stream in [same_file::Handle::stdout(), same_file::Handle::stderr()]
            .into_iter()
            .flatten()
        {
            anyhow::ensure!(
                handle != stream,
                "log `{}` points to stdout or stderr; choose a separate file",
                display_path(self.path())
            );
        }
        let unchanged = std::fs::canonicalize(self.path()).with_context(|| {
            format!(
                "cannot use log `{}`; choose a new file",
                display_path(self.path())
            )
        })?;
        if unchanged != self.path() {
            bail!(
                "log path `{}` changed before it could be opened; choose a new file",
                display_path(self.path())
            );
        }
        validate_contents(self.path(), &mut file)?;
        for record in &startup.records {
            file.write_all(record).with_context(|| {
                format!(
                    "cannot write log `{}`; choose a writable location for `--log-file`",
                    display_path(self.path())
                )
            })?;
        }
        if startup.omitted_records != 0 {
            file.write_all(&record_bytes(
                "WARN",
                "tola::log",
                json!({
                    "kind": "startup_omitted", "omitted": startup.omitted_records,
                    "message": "early log records exceeded the startup buffer",
                }),
            ))
            .with_context(|| {
                format!(
                    "cannot write log `{}`; choose a writable location for `--log-file`",
                    display_path(self.path())
                )
            })?;
        }
        Ok(file)
    }

    pub(crate) fn record(&self, level: &str, target: &str, fields: Value) {
        self.write_event(&record_bytes(level, target, fields));
    }

    pub(crate) fn is_pending(&self) -> bool {
        matches!(
            *self
                .inner
                .destination
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            LogDestination::Pending(_)
        )
    }

    pub(crate) fn writer(&self) -> LogWriter {
        LogWriter {
            file: self.clone(),
            bytes: Vec::new(),
        }
    }

    fn write_event(&self, bytes: &[u8]) {
        let failure = {
            let mut destination = self
                .inner
                .destination
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &mut *destination {
                LogDestination::Pending(startup) => {
                    startup.push(bytes);
                    None
                }
                LogDestination::Recording { file, stopped } => {
                    if *stopped {
                        None
                    } else {
                        let failure = file.write_all(bytes).err();
                        if failure.is_some() {
                            *stopped = true;
                        }
                        failure
                    }
                }
                LogDestination::Unavailable => None,
            }
        };
        if let Some(error) = failure {
            (self.inner.on_write_failure)(self.path(), &error);
        }
    }

    /// Best-effort recording without waiting for another writer or emitting tracing.
    ///
    /// Reports whether the record reached the log: the panic report may only name a log that
    /// really holds it.
    pub(crate) fn record_panic(&self, panic: &std::panic::PanicHookInfo<'_>) -> bool {
        let Ok(mut destination) = self.inner.destination.try_lock() else {
            return false;
        };
        let LogDestination::Recording {
            file,
            stopped: false,
        } = &mut *destination
        else {
            return false;
        };
        let message = panic
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        let location = panic.location().map(|location| {
            json!({
                "file": location.file(), "line": location.line(), "column": location.column(),
            })
        });
        let record = record_bytes(
            "ERROR",
            "tola::panic",
            json!({
                "kind": "panic", "message": message, "location": location,
                "backtrace": std::backtrace::Backtrace::force_capture().to_string(),
            }),
        );
        file.write_all(&record).is_ok()
    }
}

fn require_regular_file(path: &Path) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => bail!(
            "log `{}` is not a file; choose a file path for `--log-file`",
            display_path(path)
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("cannot use log `{}`", display_path(path)))
        }
    }
}

/// Require `file` to still be what `path` names, and the only link to it.
///
/// Another session can replace this path between the checks that precede it and the open, so the
/// accepted handle is compared with what the path names before any record is written; otherwise
/// this command would record into a file its announced path no longer names.
fn require_path_names_file(path: &Path, file: &File, handle: &same_file::Handle) -> Result<()> {
    // The named handle is built with this module's hardened open: a path swapped for a FIFO or a
    // link must be refused, not waited on or followed.
    let named = crate::sys::open_log_for_read(path)
        .and_then(same_file::Handle::from_file)
        .with_context(|| format!("cannot use log `{}`; choose a new file", display_path(path)))?;
    anyhow::ensure!(
        *handle == named,
        "log path `{}` no longer names the opened file; choose a new file",
        display_path(path)
    );
    require_single_link(path, file)
}

fn require_single_link(path: &Path, file: &File) -> Result<()> {
    let links = crate::sys::link_count(file)
        .with_context(|| format!("cannot use log `{}`; choose a new file", display_path(path)))?;
    anyhow::ensure!(
        links == 1,
        "log `{}` is linked from more than one path; choose a new file",
        display_path(path)
    );
    Ok(())
}

fn validate_contents(path: &Path, file: &mut File) -> Result<()> {
    let shown = display_path(path);
    let length = file
        .metadata()
        .with_context(|| format!("cannot use log `{shown}`; choose a new file"))?
        .len();
    if length == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("cannot read log `{shown}`; choose a new file"))?;
    let mut first = String::new();
    BufReader::new((&mut *file).take(MAX_LOG_HEADER_BYTES))
        .read_line(&mut first)
        .with_context(|| format!("cannot read log `{shown}`; choose a new file"))?;
    let record = serde_json::from_str::<Value>(&first).ok();
    if !record
        .as_ref()
        .is_some_and(|record| record["fields"]["kind"] == "command_started")
    {
        bail!("log `{shown}` is not a Tola command log; choose a new file");
    }
    file.seek(SeekFrom::End(-1))
        .with_context(|| format!("cannot read log `{shown}`; choose a new file"))?;
    let mut last = [0];
    file.read_exact(&mut last)
        .with_context(|| format!("cannot read log `{shown}`; choose a new file"))?;
    if last[0] != b'\n' {
        bail!("log `{shown}` has an incomplete final record; choose a new file");
    }
    Ok(())
}

/// One record line under the JSON envelope every record of the log file shares.
///
/// The file receives records from two producers: events from `tracing` and records the command
/// boundary hands to [`LogFile::record`]. Both name the process, so a log that concurrent commands
/// append to stays attributable, and both keep their own fields under `fields`.
///
/// `spans` carries an event's span scope when it ran inside one: the chain from its root span to
/// the span the event ran in, each a JSON object the JSON field formatter already wrote. The
/// fragments go through as written, so their escaping and key order survive.
fn record_line(level: &str, target: &str, spans: Option<&[String]>, fields: &str) -> String {
    let mut line = String::new();
    write!(
        line,
        "{{\"timestamp\":{},\"level\":{},\"target\":{},\"pid\":{}",
        Value::String(utc_stamp()),
        Value::String(level.to_owned()),
        Value::String(target.to_owned()),
        std::process::id(),
    )
    .expect("writing to a String succeeds");
    if let Some((current, ancestors)) = spans.and_then(|spans| spans.split_last()) {
        write!(line, ",\"span\":{current},\"spans\":[").expect("writing to a String succeeds");
        for (index, span) in ancestors.iter().enumerate() {
            if index != 0 {
                line.push(',');
            }
            line.push_str(span);
        }
        if !ancestors.is_empty() {
            line.push(',');
        }
        line.push_str(current);
        line.push(']');
    }
    line.push_str(",\"fields\":");
    line.push_str(fields);
    line.push_str("}\n");
    line
}

fn record_bytes(level: &str, target: &str, fields: Value) -> Vec<u8> {
    record_line(level, target, None, &fields.to_string()).into_bytes()
}

pub(crate) struct LogWriter {
    file: LogFile,
    bytes: Vec<u8>,
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        if !self.bytes.is_empty() {
            self.file.write_event(&self.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log this session stops must not open again and must not keep writing.
    #[test]
    fn disabled_log_drops_later_records() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("session.jsonl");
        let log = LogFile::prepare_session(&path, unexpected_write_failure).unwrap();
        started(&log);
        assert!(log.start().unwrap());

        // The first refusal changes the state; a later round has nothing left to report.
        assert!(log.disable());
        assert!(!log.disable());
        assert!(log.is_stopped());
        assert!(!log.start().unwrap());

        log.record("INFO", "tola::terminal", json!({"kind": "late"}));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("late"));
    }

    fn unexpected_write_failure(path: &Path, error: &io::Error) {
        panic!(
            "unexpected log write failure at `{}`: {error}",
            path.display()
        );
    }

    fn started(log: &LogFile) {
        log.record("INFO", "tola::terminal", json!({"kind": "command_started"}));
    }

    fn read_records(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// One record line with the timestamp and process id replaced by placeholders, so a test can
    /// assert the bytes the envelope writes around them.
    pub(super) fn normalized_record(bytes: &[u8]) -> String {
        let line = std::str::from_utf8(bytes).expect("log records are UTF-8");
        let (head, rest) = line
            .split_once("\"timestamp\":\"")
            .expect("every record names its time");
        let (timestamp, rest) = rest.split_once('"').expect("a timestamp is a JSON string");
        assert!(!timestamp.is_empty(), "{line}");
        let (middle, rest) = rest
            .split_once("\"pid\":")
            .expect("every record names its process");
        let end = rest
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(rest.len());
        assert!(
            end != 0 && rest[..end].chars().all(|c| c.is_ascii_digit()),
            "{line}"
        );
        format!(
            "{head}\"timestamp\":\"<timestamp>\"{middle}\"pid\":<pid>{}",
            &rest[end..]
        )
    }

    #[test]
    fn command_record_keeps_envelope_bytes() {
        let record = record_bytes(
            "INFO",
            "tola::test",
            json!({"kind": "pinned", "detail": "value"}),
        );
        assert_eq!(
            normalized_record(&record),
            "{\"timestamp\":\"<timestamp>\",\"level\":\"INFO\",\"target\":\"tola::test\",\"pid\":<pid>,\"fields\":{\"kind\":\"pinned\",\"detail\":\"value\"}}\n"
        );
    }

    #[test]
    fn preparing_log_creates_no_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("site");
        let log =
            LogFile::prepare(&target.join(".tola/init.jsonl"), unexpected_write_failure).unwrap();
        started(&log);
        assert!(!target.exists());
        assert!(log.start().unwrap());
        assert!(!log.start().unwrap());
        assert!(log.path().is_file());
    }

    #[test]
    fn sessions_append_early_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        for _ in 0..2 {
            let log = LogFile::prepare(&path, unexpected_write_failure).unwrap();
            started(&log);
            log.record(
                "ERROR",
                "tola::diagnostic",
                json!({
                    "kind": "diagnostic",
                    "diagnostic": {"code": "config.invalid", "message": "invalid configuration"},
                }),
            );
            log.start().unwrap();
        }
        let records = read_records(&path);
        assert_eq!(records.len(), 4);
        assert_eq!(records[1]["fields"]["diagnostic"]["code"], "config.invalid");
    }

    #[test]
    fn startup_buffer_evicts_oldest_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let log = LogFile::prepare(&path, unexpected_write_failure).unwrap();
        started(&log);
        for sequence in 1..=3 {
            log.record(
                "INFO",
                "tola::terminal",
                json!({"sequence": sequence, "message": "x".repeat(MAX_BUFFERED_STARTUP_BYTES / 2)}),
            );
        }
        log.record(
            "INFO",
            "tola::terminal",
            json!({"message": "x".repeat(MAX_BUFFERED_STARTUP_BYTES)}),
        );
        log.start().unwrap();

        let records = read_records(&path);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["fields"]["kind"], "command_started");
        assert_eq!(records[1]["fields"]["sequence"], 3);
        assert_eq!(records[2]["fields"]["kind"], "startup_omitted");
        assert_eq!(records[2]["fields"]["omitted"], 3);
    }

    #[test]
    fn source_files_stay_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, "[site]\ntitle = 'Keep me'\n").unwrap();
        assert!(LogFile::prepare(&path, unexpected_write_failure).is_err());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "[site]\ntitle = 'Keep me'\n"
        );
    }

    #[test]
    fn hard_links_cannot_alias_log() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("site.typ");
        let alias = directory.path().join("session.jsonl");
        let contents = "{\"fields\":{\"kind\":\"command_started\"}}\n";
        std::fs::write(&source, contents).unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        assert!(LogFile::prepare(&alias, unexpected_write_failure).is_err());
        assert_eq!(std::fs::read_to_string(source).unwrap(), contents);
    }

    /// The opened log file and the same-file handle that must keep naming it.
    fn opened_log(path: &Path) -> (File, same_file::Handle) {
        let file = File::open(path).unwrap();
        let handle = same_file::Handle::from_file(file.try_clone().unwrap()).unwrap();
        (file, handle)
    }

    /// A path replaced while its log is being opened must not be recorded into.
    #[test]
    fn replaced_log_path_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let (file, handle) = opened_log(&path);
        // The path is moved aside and recreated while this handle still refers to the first file;
        // moving keeps that file alive, so its inode cannot be reused by the replacement.
        std::fs::rename(&path, directory.path().join("moved.jsonl")).unwrap();
        std::fs::write(&path, "{}\n").unwrap();

        assert!(require_path_names_file(&path, &file, &handle).is_err());
    }

    /// A log that gained a second link after the early check must be refused at the opened-file
    /// boundary too.
    #[test]
    fn log_linked_after_validation_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let (file, handle) = opened_log(&path);
        std::fs::hard_link(&path, directory.path().join("alias.jsonl")).unwrap();

        assert!(require_path_names_file(&path, &file, &handle).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn device_files_are_refused() {
        assert!(LogFile::prepare(Path::new("/dev/null"), unexpected_write_failure).is_err());
    }

    /// A FIFO at the log path is refused rather than waited on for a writer.
    #[cfg(unix)]
    #[test]
    fn fifo_log_path_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let fifo = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(fifo.success(), "{fifo:?}");

        assert!(LogFile::prepare(&path, unexpected_write_failure).is_err());
    }

    /// A FIFO that replaced the log path after its early check is refused without waiting for a
    /// writer on it.
    #[cfg(unix)]
    #[test]
    fn fifo_replacing_the_log_path_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let (file, handle) = opened_log(&path);
        std::fs::remove_file(&path).unwrap();
        let fifo = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(fifo.success(), "{fifo:?}");

        assert!(require_path_names_file(&path, &file, &handle).is_err());
    }

    #[test]
    fn write_failure_reports_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, "").unwrap();
        let failures = Arc::new(Mutex::new(Vec::new()));
        let reported = Arc::clone(&failures);
        let log = LogFile {
            inner: Arc::new(LogFileInner {
                path: path.clone(),
                declared_path: path.clone(),
                origin: super::LogOrigin::Explicit,
                destination: Mutex::new(LogDestination::Recording {
                    file: File::open(&path).unwrap(),
                    stopped: false,
                }),
                on_write_failure: Box::new(move |path, error| {
                    reported
                        .lock()
                        .unwrap()
                        .push((path.to_owned(), error.to_string()));
                }),
            }),
        };
        log.write_event(b"first\n");
        log.write_event(b"second\n");
        let failures = failures.lock().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, path);
        assert!(!failures[0].1.is_empty());
        assert!(std::fs::read(path).unwrap().is_empty());
    }
}
