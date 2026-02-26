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
//! log!("build"; "compiling {} files", count);
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
use owo_colors::OwoColorize;
use parking_lot::Mutex;
use std::{
    io::{Write, stdout},
    sync::LazyLock,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Global verbose flag (set by --verbose CLI argument)
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Set verbose mode globally
pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::SeqCst);
}

/// Check if verbose mode is enabled
#[allow(dead_code)] // Used by debug! macro
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

/// Active progress bar count (for log coordination)
static BAR_COUNT: AtomicUsize = AtomicUsize::new(0);

// ============================================================================
// Log Macro
// ============================================================================

/// Log a message with a colored module prefix
///
/// # Usage
/// ```ignore
/// log!("module"; "message with {} formatting", args);
/// ```
#[macro_export]
macro_rules! log {
    ($module:expr; $($arg:tt)*) => {{
        $crate::logger::log($module, &format!($($arg)*))
    }};
}

/// Log a debug message (only shown when --verbose is enabled)
///
/// # Usage
/// ```ignore
/// debug!("module"; "debug info: {}", value);
/// ```
#[macro_export]
macro_rules! debug {
    ($module:expr; $($arg:tt)*) => {{
        if $crate::logger::is_verbose() {
            $crate::logger::log($module, &format!($($arg)*))
        }
    }};
}

/// Execute code only when --verbose is enabled
///
/// Use this to avoid computing expensive debug data when not needed.
///
/// # Usage
/// ```ignore
/// debug_do! {
///     let summary = expensive_computation();
///     debug!("module"; "result: {:?}", summary);
/// }
/// ```
#[macro_export]
macro_rules! debug_do {
    ($($body:tt)*) => {{
        if $crate::logger::is_verbose() {
            $($body)*
        }
    }};
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Log a message with a colored module prefix
#[inline]
#[allow(clippy::cast_possible_truncation)] // Safe: bars count is always small
pub fn log(module: &str, message: &str) {
    let module_lower = module.to_ascii_lowercase();
    let prefix = colorize_prefix(module, &module_lower);

    let mut stdout = stdout().lock();

    let bar_count = BAR_COUNT.load(Ordering::SeqCst);
    if bar_count > 0 {
        execute!(stdout, cursor::MoveUp(bar_count as u16)).ok();
        execute!(stdout, Clear(ClearType::FromCursorDown)).ok();
    } else {
        execute!(stdout, Clear(ClearType::UntilNewLine)).ok();
    }

    writeln!(stdout, "{prefix} {message}").ok();

    if bar_count > 0 {
        for _ in 0..bar_count {
            writeln!(stdout).ok();
        }
    }

    stdout.flush().ok();
}

/// Apply color to a module prefix based on module type
#[inline]
fn colorize_prefix(module: &str, module_lower: &str) -> String {
    let prefix = format!("[{module}]");
    match module_lower {
        "serve" => prefix.bright_blue().bold().to_string(),
        "watch" => prefix.bright_green().bold().to_string(),
        "error" => prefix.bright_red().bold().to_string(),
        _ => prefix.bright_yellow().bold().to_string(),
    }
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

/// Single-line status display for watch mode
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
pub struct WatchStatus {
    /// Lines of previous output to clear
    last_lines: usize,
}

/// Global watch status display shared across watch-mode subsystems.
///
/// This allows scan/build/reload phases to overwrite each other's status block
/// instead of leaving stale error blocks in terminal.
static WATCH_STATUS: LazyLock<Mutex<WatchStatus>> =
    LazyLock::new(|| Mutex::new(WatchStatus::new()));

impl WatchStatus {
    /// Create a new watch status display.
    pub const fn new() -> Self {
        Self { last_lines: 0 }
    }

    /// Display success message (✓ prefix, green).
    pub fn success(&mut self, message: &str) {
        self.display(format!("{}", "✓".green()), message);
    }

    /// Display unchanged message (dimmed, no symbol).
    pub fn unchanged(&mut self, message: &str) {
        self.display(String::new(), &format!("{}", message.dimmed()));
    }

    /// Display error message (✗ prefix, red) with optional detail.
    pub fn error(&mut self, summary: &str, detail: &str) {
        let message = if detail.is_empty() {
            summary.to_string()
        } else {
            format!("{summary}\n{detail}")
        };
        self.display(format!("{}", "✗".red()), &message);
    }

    /// Display warning message (⚠ prefix, yellow) with detail.
    pub fn warning(&mut self, detail: &str) {
        self.display(format!("{}", "⚠".yellow()), detail);
    }

    /// Internal display logic with line overwriting.
    ///
    /// ALL messages (success, unchanged, error) are tracked and can be
    /// overwritten by the next message. This ensures a clean single-block
    /// status display in watch mode.
    fn display(&mut self, symbol: String, message: &str) {
        let mut stdout = stdout().lock();

        // Clear previous output by moving cursor up and clearing
        if self.last_lines > 0 {
            #[allow(clippy::cast_possible_truncation)]
            let lines = self.last_lines as u16;
            execute!(stdout, cursor::MoveUp(lines)).ok();
            execute!(stdout, Clear(ClearType::FromCursorDown)).ok();
        }

        // Format message with timestamp
        let timestamp = format!("[{}]", now()).dimmed().to_string();
        let line = if symbol.is_empty() {
            format!("{timestamp} {message}")
        } else {
            format!("{timestamp} {symbol} {message}")
        };

        // Print and count lines
        writeln!(stdout, "{line}").ok();
        stdout.flush().ok();

        // Track actual line count (including newlines in message)
        self.last_lines = message.matches('\n').count() + 1;
    }

    /// Clear the status line.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        if self.last_lines > 0 {
            let mut stdout = stdout().lock();
            #[allow(clippy::cast_possible_truncation)]
            let lines = self.last_lines as u16;
            execute!(stdout, cursor::MoveUp(lines)).ok();
            execute!(stdout, Clear(ClearType::FromCursorDown)).ok();
            stdout.flush().ok();
            self.last_lines = 0;
        }
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

        BAR_COUNT.store(1, Ordering::SeqCst);

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

        let mut stdout = stdout().lock();
        // Clear line and write progress (no newline - stays on same line)
        execute!(
            stdout,
            cursor::MoveToColumn(0),
            Clear(ClearType::CurrentLine)
        )
        .ok();
        write!(stdout, "{} {}", prefix, line).ok();
        stdout.flush().ok();
    }

    /// Finish progress display, preserve line and move to next line.
    pub fn finish(self) {
        BAR_COUNT.store(0, Ordering::SeqCst);

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

            let mut stdout = stdout().lock();
            // Final line with newline to preserve it
            execute!(
                stdout,
                cursor::MoveToColumn(0),
                Clear(ClearType::CurrentLine)
            )
            .ok();
            writeln!(stdout, "{} {}", prefix, line).ok();
            stdout.flush().ok();
        }

        std::mem::forget(self); // Prevent Drop from clearing
    }
}

impl Drop for ProgressLine {
    fn drop(&mut self) {
        BAR_COUNT.store(0, Ordering::SeqCst);

        // Clear the line on drop (if not finished properly)
        let mut stdout = stdout().lock();
        execute!(
            stdout,
            cursor::MoveToColumn(0),
            Clear(ClearType::CurrentLine)
        )
        .ok();
        stdout.flush().ok();
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
        let status = WatchStatus::new();
        assert_eq!(status.last_lines, 0);
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
