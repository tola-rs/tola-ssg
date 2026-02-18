//! Pluralization utilities.

/// Return "s" suffix for plural counts
///
/// # Examples
///
/// - `plural_s(0)` -> `"s"` (0 files)
/// - `plural_s(1)` -> `""` (1 file)
/// - `plural_s(5)` -> `"s"` (5 files)
#[inline]
pub fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Format count with noun, handling pluralization
///
/// # Examples
///
/// - `plural_count(0, "file")` -> `"0 files"`
/// - `plural_count(1, "file")` -> `"1 file"`
/// - `plural_count(5, "file")` -> `"5 files"`
#[inline]
pub fn plural_count(count: usize, noun: &str) -> String {
    format!("{} {}{}", count, noun, plural_s(count))
}
