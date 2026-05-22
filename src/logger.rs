//! Logging utilities with colored output and progress display.
//!
//! This module provides:
//! - `log!` macro for formatted terminal output with colored prefixes
//! - `ProgressLine` for single-line progress display with multiple counters
//! - `WatchStatus` for watch mode status messages
//!
//! # Example
//!
//! ```ignore
//! // Simple logging
//! logger::log("build", format_args!("compiling {} files", count));
//!
//! // Progress line for build
//! let progress = ProgressLine::new(&[("typst", 69), ("markdown", 10)]);
//! progress.inc("typst");
//! progress.finish();
//! ```

use crossterm::{
    cursor, execute,
    terminal::{Clear, ClearType},
};
use owo_colors::{OwoColorize, Stream};
use parking_lot::Mutex;
use std::{
    fmt::Display,
    io::{IsTerminal, Write, stdout},
    sync::LazyLock,
    sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
};

/// Global verbose flag (set by --verbose CLI argument)
static VERBOSE: AtomicBool = AtomicBool::new(false);
static COLOR_MODE: AtomicU8 = AtomicU8::new(ColorMode::Auto as u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto = 0,
    Always = 1,
    Never = 2,
}

/// Set verbose mode globally
pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::SeqCst);
}

/// Check if verbose mode is enabled
#[allow(dead_code)] // Used by debug! macro
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

pub fn set_color_mode(mode: ColorMode) {
    COLOR_MODE.store(mode as u8, Ordering::SeqCst);
}

pub fn colors_enabled() -> bool {
    match COLOR_MODE.load(Ordering::SeqCst) {
        value if value == ColorMode::Always as u8 => true,
        value if value == ColorMode::Never as u8 => false,
        _ => {
            format!(
                "{}",
                "x".if_supports_color(Stream::Stdout, |text| text.red())
            ) != "x"
        }
    }
}

pub fn terminal_control_enabled() -> bool {
    stdout().is_terminal()
}

static OUTPUT: LazyLock<Mutex<OutputState>> = LazyLock::new(|| Mutex::new(OutputState::new()));

enum Transient {
    Progress(String),
    Status(String),
}

impl Transient {
    fn line_count(&self) -> usize {
        match self {
            Self::Progress(_) => 1,
            Self::Status(text) => line_count(text),
        }
    }
}

struct OutputState {
    transient: Option<Transient>,
}

impl OutputState {
    const fn new() -> Self {
        Self { transient: None }
    }

    fn write_persistent(&mut self, text: &str) {
        let mut stdout = stdout().lock();
        if terminal_control_enabled() {
            self.clear_transient(&mut stdout);
        }
        writeln!(stdout, "{text}").ok();
        if terminal_control_enabled() {
            self.redraw_transient(&mut stdout);
        }
        stdout.flush().ok();
    }

    fn set_progress(&mut self, text: String) {
        if !terminal_control_enabled() {
            self.transient = Some(Transient::Progress(text));
            return;
        }

        let mut stdout = stdout().lock();
        self.clear_transient(&mut stdout);
        self.transient = Some(Transient::Progress(text));
        self.redraw_transient(&mut stdout);
        stdout.flush().ok();
    }

    fn finish_progress(&mut self, text: &str) {
        if !terminal_control_enabled() {
            if matches!(self.transient, Some(Transient::Progress(_))) {
                self.transient = None;
            }
            return;
        }

        let mut stdout = stdout().lock();
        if matches!(self.transient, Some(Transient::Progress(_))) {
            self.clear_transient(&mut stdout);
            self.transient = None;
        }
        writeln!(stdout, "{text}").ok();
        stdout.flush().ok();
    }

    fn clear_progress(&mut self) {
        if !terminal_control_enabled() {
            if matches!(self.transient, Some(Transient::Progress(_))) {
                self.transient = None;
            }
            return;
        }

        let mut stdout = stdout().lock();
        if matches!(self.transient, Some(Transient::Progress(_))) {
            self.clear_transient(&mut stdout);
            self.transient = None;
            stdout.flush().ok();
        }
    }

    fn set_status(&mut self, text: String) {
        if !terminal_control_enabled() {
            self.write_persistent(&text);
            return;
        }

        let mut stdout = stdout().lock();
        self.clear_transient(&mut stdout);
        self.transient = Some(Transient::Status(text));
        self.redraw_transient(&mut stdout);
        stdout.flush().ok();
    }

    fn clear_status(&mut self) {
        if !terminal_control_enabled() {
            if matches!(self.transient, Some(Transient::Status(_))) {
                self.transient = None;
            }
            return;
        }

        let mut stdout = stdout().lock();
        if matches!(self.transient, Some(Transient::Status(_))) {
            self.clear_transient(&mut stdout);
            self.transient = None;
            stdout.flush().ok();
        }
    }

    fn clear_transient(&self, stdout: &mut impl Write) {
        let Some(transient) = &self.transient else {
            execute!(
                stdout,
                cursor::MoveToColumn(0),
                Clear(ClearType::UntilNewLine)
            )
            .ok();
            return;
        };

        match transient {
            Transient::Progress(_) => {
                execute!(
                    stdout,
                    cursor::MoveToColumn(0),
                    Clear(ClearType::CurrentLine)
                )
                .ok();
            }
            Transient::Status(_) => {
                #[allow(clippy::cast_possible_truncation)]
                let lines = transient.line_count() as u16;
                execute!(stdout, cursor::MoveUp(lines)).ok();
                execute!(stdout, cursor::MoveToColumn(0)).ok();
                execute!(stdout, Clear(ClearType::FromCursorDown)).ok();
            }
        }
    }

    fn redraw_transient(&self, stdout: &mut impl Write) {
        match &self.transient {
            Some(Transient::Progress(text)) => {
                write!(stdout, "{text}").ok();
            }
            Some(Transient::Status(text)) => {
                writeln!(stdout, "{text}").ok();
            }
            None => {}
        }
    }
}

fn line_count(text: &str) -> usize {
    text.lines().count().max(1)
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Log a message with a colored module prefix
pub fn log(module: &str, message: impl Display) {
    let module_lower = module.to_ascii_lowercase();
    let prefix = colorize_prefix(module, &module_lower);
    OUTPUT
        .lock()
        .write_persistent(&format!("{prefix} {message}"));
}

/// Log a verbose message with a colored module prefix.
pub fn debug(module: &str, message: impl Display) {
    if is_verbose() {
        log(module, message);
    }
}

/// Write a persistent block with a colored module prefix on the first line.
#[inline]
pub fn block(module: &str, title: &str, body: &str) {
    let module_lower = module.to_ascii_lowercase();
    let prefix = colorize_prefix(module, &module_lower);
    let text = if body.is_empty() {
        format!("{prefix} {title}")
    } else {
        format!("{prefix} {title}\n{body}")
    };
    OUTPUT.lock().write_persistent(&text);
}

/// Write persistent text without adding a module prefix.
#[inline]
pub fn text(message: &str) {
    OUTPUT.lock().write_persistent(message);
}

/// Write one persistent blank line.
#[inline]
pub fn blank() {
    OUTPUT.lock().write_persistent("");
}

/// Apply color to a module prefix based on module type
#[inline]
fn colorize_prefix(module: &str, module_lower: &str) -> String {
    let prefix = format!("[{module}]");
    if !colors_enabled() {
        return prefix;
    }
    match module_lower {
        "serve" => prefix.bright_blue().bold().to_string(),
        "watch" => prefix.bright_green().bold().to_string(),
        "error" => prefix.bright_red().bold().to_string(),
        _ => prefix.bright_yellow().bold().to_string(),
    }
}

pub fn style_error(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.red().to_string()
    } else {
        text.to_string()
    }
}

pub fn style_error_strong(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.red().bold().to_string()
    } else {
        text.to_string()
    }
}

pub fn style_warning(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.yellow().to_string()
    } else {
        text.to_string()
    }
}

pub fn style_hint(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.cyan().to_string()
    } else {
        text.to_string()
    }
}

pub fn style_success(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.green().to_string()
    } else {
        text.to_string()
    }
}

pub fn style_path(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.cyan().to_string()
    } else {
        text.to_string()
    }
}

pub fn style_field(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.bright_blue().to_string()
    } else {
        text.to_string()
    }
}

pub fn style_dim(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if colors_enabled() {
        text.dimmed().to_string()
    } else {
        text.to_string()
    }
}

fn success_symbol() -> String {
    style_success("✓")
}

fn error_symbol() -> String {
    style_error("✗")
}

fn warning_symbol() -> String {
    style_warning("⚠")
}

// ============================================================================
// Watch Status (single-line status with overwrite)
// ============================================================================

/// Get current time formatted as HH:MM:SS
fn now() -> String {
    use std::time::SystemTime;
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Convert to local time (UTC+8 for now, good enough for display)
    let local_secs = secs + 8 * 3600;
    let hours = (local_secs / 3600) % 24;
    let minutes = (local_secs / 60) % 60;
    let seconds = local_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Single status block display for watch mode
///
/// Displays status messages that overwrite the previous output,
/// keeping the terminal clean. Supports timestamps and different
/// status types (success, error, unchanged)
///
/// # Example
///
/// ```ignore
/// let mut status = WatchStatus::new();
/// status.success("rebuilt: content/index.typ");
/// status.unchanged("content/about.typ");
/// status.error("failed", "syntax error on line 5");
/// ```
pub struct WatchStatus;

/// Global watch status display shared across watch-mode subsystems.
///
/// This allows scan/build/reload phases to overwrite each other's status block
/// instead of leaving stale error blocks in terminal.
static WATCH_STATUS: LazyLock<Mutex<WatchStatus>> =
    LazyLock::new(|| Mutex::new(WatchStatus::new()));

impl WatchStatus {
    /// Create a new watch status display.
    pub const fn new() -> Self {
        Self
    }

    /// Display success message (✓ prefix, green).
    pub fn success(&mut self, message: &str) {
        self.display(success_symbol(), message);
    }

    /// Display unchanged message (dimmed, no symbol).
    pub fn unchanged(&mut self, message: &str) {
        self.display(String::new(), &style_dim(message));
    }

    /// Display error message (✗ prefix, red) with optional detail.
    pub fn error(&mut self, summary: &str, detail: &str) {
        let message = if detail.is_empty() {
            summary.to_string()
        } else {
            format!("{summary}\n{detail}")
        };
        self.display(error_symbol(), &message);
    }

    /// Display warning message (⚠ prefix, yellow) with detail.
    pub fn warning(&mut self, detail: &str) {
        self.display(warning_symbol(), detail);
    }

    /// Internal display logic with line overwriting.
    ///
    /// ALL messages (success, unchanged, error) are tracked and can be
    /// overwritten by the next message. This ensures a clean single-block
    /// status display in watch mode.
    fn display(&mut self, symbol: String, message: &str) {
        let timestamp = style_dim(format!("[{}]", now()));
        let line = if symbol.is_empty() {
            format!("{timestamp} {message}")
        } else {
            format!("{timestamp} {symbol} {message}")
        };
        OUTPUT.lock().set_status(line);
    }

    /// Clear the status line.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        OUTPUT.lock().clear_status();
    }
}

/// Global watch status: success
pub fn status_success(message: &str) {
    WATCH_STATUS.lock().success(message);
}

/// Global watch status: unchanged
#[allow(dead_code)]
pub fn status_unchanged(message: &str) {
    WATCH_STATUS.lock().unchanged(message);
}

/// Global watch status: error
pub fn status_error(summary: &str, detail: &str) {
    WATCH_STATUS.lock().error(summary, detail);
}

/// Global watch status: warning
pub fn status_warning(detail: &str) {
    WATCH_STATUS.lock().warning(detail);
}

// ============================================================================
// Progress Line (single-line counters)
// ============================================================================

/// Single-line progress display with multiple counters
///
/// Displays: `[build] typst(42/69) markdown(5/10) assets(120/371)`
///
/// All counters update in place on the same line. Uses `try_lock` to avoid
/// blocking worker threads - if display is busy, the update is skipped
///
/// # Example
///
/// ```ignore
/// let progress = ProgressLine::new(&[
///     ("typst", 69),
///     ("markdown", 10),
///     ("assets", 371),
/// ]);
/// progress.inc("typst");
/// progress.inc("assets");
/// progress.finish(); // keeps the line, moves cursor down
/// ```
pub struct ProgressLine {
    counters: Vec<Counter>,
    lock: Mutex<()>,
}

struct Counter {
    name: &'static str,
    total: usize,
    current: AtomicUsize,
}

impl ProgressLine {
    /// Create a new build progress display.
    ///
    /// Only includes counters with total > 0.
    pub fn new(items: &[(&'static str, usize)]) -> Self {
        let counters: Vec<_> = items
            .iter()
            .filter(|(_, total)| *total > 0)
            .map(|(name, total)| Counter {
                name,
                total: *total,
                current: AtomicUsize::new(0),
            })
            .collect();

        let progress = Self {
            counters,
            lock: Mutex::new(()),
        };
        progress.display();
        progress
    }

    /// Increment the counter with the given name.
    ///
    /// Non-blocking: if display lock is held, skips refresh.
    #[inline]
    pub fn inc(&self, name: &str) {
        for counter in &self.counters {
            if counter.name == name {
                counter.current.fetch_add(1, Ordering::Relaxed);
                // Non-blocking: skip display if lock is held
                if self.lock.try_lock().is_some() {
                    self.display();
                }
                return;
            }
        }
    }

    /// Display the current progress line (overwrites current line with \r).
    fn display(&self) {
        let mut parts = Vec::with_capacity(self.counters.len());
        for counter in &self.counters {
            let current = counter.current.load(Ordering::Relaxed);
            parts.push(format!("{}({}/{})", counter.name, current, counter.total));
        }

        let line = parts.join(" ");
        let prefix = colorize_prefix("build", "build");

        OUTPUT.lock().set_progress(format!("{} {}", prefix, line));
    }

    /// Finish progress display, preserve line and move to next line.
    pub fn finish(self) {
        {
            let _guard = self.lock.lock(); // Wait for any pending display

            // Final display with correct counts
            let mut parts = Vec::with_capacity(self.counters.len());
            for counter in &self.counters {
                let current = counter.current.load(Ordering::Relaxed);
                parts.push(format!("{}({}/{})", counter.name, current, counter.total));
            }
            let line = parts.join(" ");
            let prefix = colorize_prefix("build", "build");
            OUTPUT
                .lock()
                .finish_progress(&format!("{} {}", prefix, line));
        }

        std::mem::forget(self); // Prevent Drop from clearing
    }
}

impl Drop for ProgressLine {
    fn drop(&mut self) {
        OUTPUT.lock().clear_progress();
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // WatchStatus tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_watch_status_new() {
        let _status = WatchStatus::new();
    }

    #[test]
    fn test_watch_status_line_count_single() {
        // Single line message should count as 1
        let message = "rebuilt: content/index";
        let count = message.matches('\n').count() + 1;
        assert_eq!(count, 1);
    }

    #[test]
    fn test_watch_status_line_count_multiline() {
        // Multi-line error message
        let message = "failed: content/index\nerror: unknown variable\n  --> line 5";
        let count = message.matches('\n').count() + 1;
        assert_eq!(count, 3);
    }

    #[test]
    fn test_watch_status_line_count_error_with_detail() {
        // Typical error format: summary + newline + detail
        let summary = "failed: content/index";
        let detail = "Typst compilation failed:\nerror: something\n  --> file:1:1";
        let message = format!("{summary}\n{detail}");
        let count = message.matches('\n').count() + 1;
        assert_eq!(count, 4); // summary + 3 lines of detail
    }
}
