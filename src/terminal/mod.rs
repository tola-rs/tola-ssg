//! Terminal presentation: console output, status text, diagnostics, pager, and the development view.

pub(crate) mod ansi;
pub(crate) mod clipboard;
pub(crate) mod code;
pub(crate) mod color;
mod console;
pub(crate) mod development;
mod diagnostic;
mod hooks;
mod limits;
mod pager;
mod path;
pub(crate) mod progress;
pub(crate) mod session;
pub(crate) mod sink;
mod source;
pub(crate) mod style;
pub(crate) mod table;
pub(crate) mod text;
pub(crate) mod ui;
pub(crate) mod wrap;

pub(crate) use console::Terminal;
pub(crate) use diagnostic::render as render_diagnostic;
pub(crate) use hooks::{EVERY_STAGE, PRE_PUBLICATION_STAGES, overview as hook_overview};
pub(crate) use pager::PagerCommand;
pub(crate) use path::{
    display_path, display_path_as_given, display_path_toward_home, display_path_within,
};
pub(crate) use progress::{
    describe_changes, describe_outputs, format_duration, plural_count, serving_line, site_summary,
};
pub(crate) use source::SourceFiles;
pub(crate) use style::Palette;

pub(crate) use session::InputCancelled;
pub(crate) use sink::{OutputSink, StdoutClosed};
use std::io::{self, IsTerminal};

const INDENT_UNIT: &str = "  ";

/// Best-effort restoration for an application panic hook.
pub(crate) fn restore() {
    session::restore();
}

/// A cancelled child cannot run its own alternate-screen and cursor cleanup.
pub(crate) fn restore_external_screen() {
    if io::stderr().is_terminal() {
        let _ = session::leave_commands(&mut io::stderr());
    } else if io::stdout().is_terminal() {
        let _ = session::leave_commands(&mut io::stdout());
    }
}

pub(crate) fn append_indent(output: &mut String, level: usize) {
    for _ in 0..level {
        output.push_str(INDENT_UNIT);
    }
}
