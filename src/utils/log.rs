//! Logging utilities with colored output and progress bars.
//!
//! This module provides:
//! - `log!` macro for formatted terminal output with colored prefixes
//! - `ProgressBars` for displaying multiple progress bars simultaneously
//!
//! # Example
//!
//! ```ignore
//! // Simple logging
//! log!("build"; "compiling {} files", count);
//!
//! // Progress bars for parallel tasks
//! let progress = ProgressBars::new(&[("content", 100), ("assets", 50)]);
//! progress.inc(0); // increment content bar
//! progress.inc(1); // increment assets bar
//! progress.finish(); // clear progress bars
//! ```

use colored::{ColoredString, Colorize};
use crossterm::{
    cursor, execute,
    terminal::{Clear, ClearType, size},
};
use std::{
    io::{Write, stdout},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Cached terminal width (fetched once on first use)
static TERMINAL_WIDTH: OnceLock<u16> = OnceLock::new();

// ============================================================================
// Layout Constants
// ============================================================================
//
// Progress bar format: "[module] [████░░░░] 42/100"
//                       ^------^ ^-------^ ^----^
//                       prefix   bar       count

/// Length of brackets around module name: "[]"
const BRACKET_LEN: usize = 2;
/// Space after prefix: "[module] " <- this space
const SPACE_AFTER_PREFIX: usize = 1;
/// Bar wrapper: " []" (space + brackets around progress bar)
const BAR_WRAPPER_LEN: usize = 3;
/// Space before count: "...] 42/100" <- this space
const SPACE_BEFORE_COUNT: usize = 1;
/// Minimum progress bar width in characters
const MIN_BAR_WIDTH: usize = 10;
/// Maximum progress bar width in characters
const MAX_BAR_WIDTH: usize = 40;

/// Calculate total prefix length for a module name.
///
/// Returns: `module.len() + 3` (for "[", "]", and trailing space)
#[inline]
const fn calc_prefix_len(module_len: usize) -> usize {
    module_len + BRACKET_LEN + SPACE_AFTER_PREFIX
}

/// Get terminal width, cached after first call.
/// Falls back to 120 columns if detection fails.
fn get_terminal_width() -> u16 {
    *TERMINAL_WIDTH.get_or_init(|| size().map(|(w, _)| w).unwrap_or(120))
}

// ============================================================================
// Log Macro
// ============================================================================

/// Log a message with a colored module prefix.
///
/// # Usage
/// ```ignore
/// log!("module"; "message with {} formatting", args);
/// ```
#[macro_export]
macro_rules! log {
    ($module:expr; $($arg:tt)*) => {{
        $crate::utils::log::log($module, &format!($($arg)*))
    }};
}

// ============================================================================
// Progress Bars
// ============================================================================

/// Manages multiple progress bars displayed on separate terminal lines.
///
/// Each bar occupies one line and updates in place using ANSI cursor control.
/// Bars are indexed by their creation order (0, 1, 2, ...).
///
/// # Thread Safety
/// Uses a mutex to synchronize terminal updates from multiple threads.
pub struct ProgressBars {
    bars: Vec<ProgressBar>,
    lock: Mutex<()>,
}

/// Internal state for a single progress bar.
struct ProgressBar {
    /// Colored prefix string (e.g., "[content]" in yellow)
    prefix: ColoredString,
    /// Pre-calculated display length of prefix
    prefix_len: usize,
    /// Total number of items to process
    total: usize,
    /// Current progress counter (atomic for thread-safe updates)
    current: AtomicUsize,
    /// Row index within the progress area (0 = first bar)
    row: usize,
}

impl ProgressBars {
    /// Create progress bars for multiple modules.
    ///
    /// # Arguments
    /// * `modules` - Slice of (module_name, total_count) tuples
    ///
    /// # Returns
    /// A new `ProgressBars` instance. Use `inc(index)` to update bars.
    ///
    /// # Example
    /// ```ignore
    /// let progress = ProgressBars::new(&[
    ///     ("content", content_files.len()),
    ///     ("assets", asset_files.len()),
    /// ]);
    /// ```
    pub fn new(modules: &[(&'static str, usize)]) -> Self {
        // Reserve terminal lines for progress bars
        let mut stdout = stdout().lock();
        for _ in 0..modules.len() {
            writeln!(stdout).ok();
        }
        stdout.flush().ok();

        let bars = modules
            .iter()
            .enumerate()
            .map(|(row, (module, total))| {
                let prefix = colorize_prefix(module, &module.to_ascii_lowercase());
                ProgressBar {
                    prefix,
                    prefix_len: calc_prefix_len(module.len()),
                    total: *total,
                    current: AtomicUsize::new(0),
                    row,
                }
            })
            .collect();

        Self {
            bars,
            lock: Mutex::new(()),
        }
    }

    /// Increment progress for the bar at the given index.
    ///
    /// Thread-safe: can be called from multiple threads simultaneously.
    #[inline]
    pub fn inc(&self, index: usize) {
        if let Some(bar) = self.bars.get(index) {
            let current = bar.current.fetch_add(1, Ordering::Relaxed) + 1;
            self.display(bar, current);
        }
    }

    /// Render a progress bar at its designated row.
    fn display(&self, bar: &ProgressBar, current: usize) {
        let _guard = self.lock.lock().ok();

        let width = get_terminal_width() as usize;

        // Calculate available width for the bar
        let progress_text = format!("{}/{}", current, bar.total);
        let overhead = bar.prefix_len + BAR_WRAPPER_LEN + SPACE_BEFORE_COUNT + progress_text.len();
        let available = width.saturating_sub(overhead);
        let bar_width = available.clamp(MIN_BAR_WIDTH, MAX_BAR_WIDTH);

        // Calculate filled/empty portions
        let filled = if bar.total > 0 {
            (current * bar_width) / bar.total
        } else {
            0
        };
        let empty = bar_width.saturating_sub(filled);

        let progress_bar: String = "█".repeat(filled) + &"░".repeat(empty);

        // Update the correct line using cursor movement
        let mut stdout = stdout().lock();
        let lines_up = self.bars.len() - bar.row;
        execute!(stdout, cursor::MoveUp(lines_up as u16)).ok();
        execute!(stdout, Clear(ClearType::CurrentLine)).ok();
        write!(stdout, "{} [{}] {}", bar.prefix, progress_bar, progress_text).ok();
        execute!(stdout, cursor::MoveDown(lines_up as u16)).ok();
        write!(stdout, "\r").ok();
        stdout.flush().ok();
    }

    /// Clear all progress bars from the terminal.
    ///
    /// Call this when processing is complete to clean up the display.
    pub fn finish(&self) {
        let _guard = self.lock.lock().ok();

        let mut stdout = stdout().lock();

        // Move to top of progress area and clear each line
        execute!(stdout, cursor::MoveUp(self.bars.len() as u16)).ok();
        for _ in &self.bars {
            execute!(stdout, Clear(ClearType::CurrentLine)).ok();
            execute!(stdout, cursor::MoveDown(1)).ok();
        }

        // Return cursor to starting position
        execute!(stdout, cursor::MoveUp(self.bars.len() as u16)).ok();
        stdout.flush().ok();
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Log a message with a colored module prefix.
///
/// Automatically truncates long messages to fit terminal width.
#[inline]
pub fn log(module: &str, message: &str) {
    let module_lower = module.to_ascii_lowercase();
    let prefix = colorize_prefix(module, &module_lower);
    let width = get_terminal_width() as usize;

    let mut stdout = stdout().lock();
    execute!(stdout, Clear(ClearType::UntilNewLine)).ok();

    // Truncate message if it exceeds available width
    let prefix_len = calc_prefix_len(module.len());
    let max_msg_len = width.saturating_sub(prefix_len);

    let message = if message.len() > max_msg_len {
        truncate_str(message, max_msg_len)
    } else {
        message
    };

    writeln!(stdout, "{prefix} {message}").ok();
    stdout.flush().ok();
}

/// Apply color to a module prefix based on module type.
#[inline]
fn colorize_prefix(module: &str, module_lower: &str) -> ColoredString {
    let prefix = format!("[{module}]");
    match module_lower {
        "serve" => prefix.bright_blue().bold(),
        "watch" => prefix.bright_green().bold(),
        "error" => prefix.bright_red().bold(),
        _ => prefix.bright_yellow().bold(),
    }
}

/// Truncate a string to fit within max_len bytes.
///
/// Ensures the result is valid UTF-8 by finding the nearest character boundary.
#[inline]
fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    // Find the last valid UTF-8 boundary within max_len
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // calc_prefix_len tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_calc_prefix_len_short_module() {
        // "a" -> "[a] " = 1 + 2 + 1 = 4
        assert_eq!(calc_prefix_len(1), 4);
    }

    #[test]
    fn test_calc_prefix_len_typical_module() {
        // "content" -> "[content] " = 7 + 2 + 1 = 10
        assert_eq!(calc_prefix_len(7), 10);
    }

    #[test]
    fn test_calc_prefix_len_empty() {
        // "" -> "[] " = 0 + 2 + 1 = 3
        assert_eq!(calc_prefix_len(0), 3);
    }

    #[test]
    fn test_calc_prefix_len_long_module() {
        // "verification" (12 chars) -> "[verification] " = 12 + 2 + 1 = 15
        assert_eq!(calc_prefix_len(12), 15);
    }

    // ------------------------------------------------------------------------
    // truncate_str tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_truncate_str_short_string() {
        // String fits within limit, return as-is
        let s = "hello";
        assert_eq!(truncate_str(s, 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact_length() {
        // String length equals limit
        let s = "hello";
        assert_eq!(truncate_str(s, 5), "hello");
    }

    #[test]
    fn test_truncate_str_needs_truncation() {
        // String exceeds limit
        let s = "hello world";
        assert_eq!(truncate_str(s, 5), "hello");
    }

    #[test]
    fn test_truncate_str_unicode_boundary() {
        // UTF-8 multibyte: "你好" is 6 bytes (3 bytes per char)
        // Truncating at byte 4 should find boundary at byte 3
        let s = "你好";
        assert_eq!(truncate_str(s, 4), "你"); // Only first char fits
    }

    #[test]
    fn test_truncate_str_unicode_exact() {
        // "你" is exactly 3 bytes
        let s = "你好";
        assert_eq!(truncate_str(s, 3), "你");
    }

    #[test]
    fn test_truncate_str_unicode_full() {
        // Both chars fit (6 bytes)
        let s = "你好";
        assert_eq!(truncate_str(s, 6), "你好");
    }

    #[test]
    fn test_truncate_str_empty() {
        let s = "";
        assert_eq!(truncate_str(s, 10), "");
    }

    #[test]
    fn test_truncate_str_zero_limit() {
        let s = "hello";
        assert_eq!(truncate_str(s, 0), "");
    }

    #[test]
    fn test_truncate_str_mixed_unicode() {
        // "a你b" = 1 + 3 + 1 = 5 bytes
        let s = "a你b";
        assert_eq!(truncate_str(s, 4), "a你"); // "a" + "你" = 4 bytes
        assert_eq!(truncate_str(s, 3), "a"); // Can't fit "你" (needs 3 bytes starting at position 1)
        assert_eq!(truncate_str(s, 2), "a"); // Only ASCII fits
    }

    // ------------------------------------------------------------------------
    // Layout constants tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_constants_values() {
        // Verify constants have expected values
        assert_eq!(BRACKET_LEN, 2); // "[" and "]"
        assert_eq!(SPACE_AFTER_PREFIX, 1); // " "
        assert_eq!(BAR_WRAPPER_LEN, 3); // " []"
        assert_eq!(SPACE_BEFORE_COUNT, 1); // " "
        assert_eq!(MIN_BAR_WIDTH, 10);
        assert_eq!(MAX_BAR_WIDTH, 40);
    }

    #[test]
    fn test_bar_width_constraints() {
        // MIN should be less than MAX
        assert!(MIN_BAR_WIDTH < MAX_BAR_WIDTH);
    }
}
