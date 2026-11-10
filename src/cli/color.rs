//! The process-wide color decision shared by first-party commands and their Nu children.

use std::sync::OnceLock;

use clap::ColorChoice;

static COLOR: OnceLock<ColorChoice> = OnceLock::new();

/// Record the process-wide color choice once, before any command runs.
pub(crate) fn install(color: ColorChoice) {
    let _ = COLOR.set(color);
}

/// Environment values that make the child Nu process match this choice.
///
/// Color detection itself stays with Nu's `UseAnsiColoring::Auto`; Tola only
/// translates its own `--color` flag into the variables that decision reads.
pub(crate) fn child_environment() -> Vec<(&'static str, String)> {
    match COLOR.get().copied().unwrap_or(ColorChoice::Auto) {
        ColorChoice::Always => vec![("FORCE_COLOR", "1".into())],
        ColorChoice::Never => vec![("NO_COLOR", "1".into())],
        ColorChoice::Auto => Vec::new(),
    }
}
