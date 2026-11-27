//! The process-wide color decision shared by first-party commands and their Nu children.

use std::sync::OnceLock;

use clap::ColorChoice;

use crate::terminal::color::{Stream, enabled};

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
        ColorChoice::Never => vec![("NO_COLOR", "1".into()), ("FORCE_COLOR", "0".into())],
        ColorChoice::Auto => Vec::new(),
    }
}

/// Environment values for a Nu hook whose streams Tola captures over pipes.
///
/// The piped worker cannot detect Tola's terminal, so `Auto` uses Tola's
/// stderr color decision and passes `FORCE_COLOR` or `NO_COLOR` to Nu.
pub(crate) fn captured_child_environment() -> Vec<(&'static str, String)> {
    let choice = COLOR.get().copied().unwrap_or(ColorChoice::Auto);
    if enabled(choice, Stream::Stderr) {
        vec![("FORCE_COLOR", "1".into())]
    } else {
        vec![("NO_COLOR", "1".into()), ("FORCE_COLOR", "0".into())]
    }
}
