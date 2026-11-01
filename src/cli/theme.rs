//! The TUI theme decision shared by first-party commands and their Nu children.

use std::sync::OnceLock;

use clap::ColorChoice;
use tola_highlight::ThemeChoice;

static THEME: OnceLock<ThemeChoice> = OnceLock::new();
static COLOR: OnceLock<ColorChoice> = OnceLock::new();

/// Decide the TUI palette from the global color choice and the environment.
pub(crate) fn resolve(color: ColorChoice) -> ThemeChoice {
    let configured = std::env::var("TOLA_THEME")
        .ok()
        .and_then(|name| ThemeChoice::parse(&name));
    if configured == Some(ThemeChoice::Plain) {
        return ThemeChoice::Plain;
    }
    let force = std::env::var_os("FORCE_COLOR").is_some_and(|value| value != "0");
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let dumb = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
    match color {
        ColorChoice::Never => ThemeChoice::Plain,
        ColorChoice::Always => configured.unwrap_or(ThemeChoice::Dark),
        ColorChoice::Auto if force => configured.unwrap_or(ThemeChoice::Dark),
        ColorChoice::Auto if no_color || dumb => ThemeChoice::Plain,
        ColorChoice::Auto => configured.unwrap_or(ThemeChoice::Dark),
    }
}

/// Record the process-wide theme once, before any command runs.
pub(crate) fn install(color: ColorChoice) {
    let _ = COLOR.set(color);
    let _ = THEME.set(resolve(color));
}

/// The installed theme; dark before installation, matching the old default.
pub(crate) fn choice() -> ThemeChoice {
    THEME.get().copied().unwrap_or(ThemeChoice::Dark)
}

/// Environment values that make the child Nu process match this theme.
///
/// Color detection itself stays with Nu's `UseAnsiColoring::Auto`; Tola only
/// translates its own `--color` flag into the variables that decision reads.
pub(crate) fn child_environment() -> Vec<(&'static str, String)> {
    let mut environment = vec![("TOLA_THEME", choice().name().to_string())];
    match COLOR.get().copied().unwrap_or(ColorChoice::Auto) {
        ColorChoice::Always => environment.push(("FORCE_COLOR", "1".into())),
        ColorChoice::Never => environment.push(("NO_COLOR", "1".into())),
        ColorChoice::Auto => {}
    }
    environment
}
