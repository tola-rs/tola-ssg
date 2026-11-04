//! The pager `tola help` documentation is written through.

use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

/// Flags a bare `less` receives: `-F` quits when one screen shows everything, `-R` passes ANSI
/// sequences through, `-X` keeps the screen on exit, `-K` quits on Ctrl+C.
const LESS_ARGUMENTS: &[&str] = &["-FRXK"];

/// A pager documentation can be written through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PagerCommand {
    program: OsString,
    arguments: Vec<OsString>,
}

impl PagerCommand {
    /// The pager this process pages documentation through.
    ///
    /// `None` when stdout is not a terminal or `TERM` is unset or `dumb`; a `less` that cannot
    /// drive the terminal stops at its own warning instead of showing the documentation.
    pub(crate) fn for_process() -> anyhow::Result<Option<Self>> {
        if !io::stdout().is_terminal() {
            return Ok(None);
        }
        let term = std::env::var_os("TERM");
        if term.is_none() || term.as_deref() == Some(OsStr::new("dumb")) {
            return Ok(None);
        }
        let less = which::which("less").ok();
        resolve(
            std::env::var_os("TOLA_PAGER").as_deref(),
            std::env::var_os("PAGER").as_deref(),
            less.as_deref(),
        )
    }

    /// Start the pager, with the documentation written to the returned handle.
    pub(crate) fn start(&self) -> io::Result<StartedPager> {
        let mut command = Command::new(&self.program);
        command.args(&self.arguments).stdin(Stdio::piped());
        // `less` reads its input charset from the environment; the documentation is UTF-8.
        if std::env::var_os("LESSCHARSET").is_none() {
            command.env("LESSCHARSET", "utf-8");
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("`Stdio::piped()` provides stdin");
        Ok(StartedPager { child, stdin })
    }

    /// The program, as the failure note names it.
    pub(crate) fn program(&self) -> &OsStr {
        &self.program
    }
}

/// A running pager whose input the documentation is written to.
pub(crate) struct StartedPager {
    child: Child,
    stdin: ChildStdin,
}

impl StartedPager {
    /// Write the documentation and wait for the pager to quit.
    pub(crate) fn write(mut self, documentation: &[u8]) -> io::Result<()> {
        let written = self.stdin.write_all(documentation);
        // Closing stdin lets the pager see end of input, so it can quit.
        drop(self.stdin);
        // `less` reports an interrupt as exit status 2; the reader's exit is not a failure.
        self.child.wait()?;
        match written {
            Ok(()) => Ok(()),
            // The reader quit before the documentation was through; there is nothing left to show.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// Decide the pager from the two variables, with the `less` on `PATH` as the fallback.
///
/// The first present variable decides: an empty or whitespace-only value, or a program whose
/// file stem is `cat`, disables paging instead of falling through to the next source.
/// Quotes group command arguments without enabling shell expansion.
fn resolve(
    tola_pager: Option<&OsStr>,
    pager: Option<&OsStr>,
    less: Option<&Path>,
) -> anyhow::Result<Option<PagerCommand>> {
    let (program, arguments): (OsString, Vec<OsString>) = match tola_pager.or(pager) {
        Some(configured) => {
            let Some(command) =
                crate::command_line::CommandLine::parse(configured).map_err(|error| {
                    anyhow::anyhow!("invalid pager command: {error}; check TOLA_PAGER or PAGER")
                })?
            else {
                return Ok(None);
            };
            if Path::new(&command.program).file_stem() == Some(OsStr::new("cat")) {
                return Ok(None);
            }
            (command.program, command.arguments)
        }
        None => match less {
            Some(less) => (less.as_os_str().to_os_string(), Vec::new()),
            None => return Ok(None),
        },
    };
    let arguments =
        if arguments.is_empty() && Path::new(&program).file_stem() == Some(OsStr::new("less")) {
            LESS_ARGUMENTS.iter().copied().map(OsString::from).collect()
        } else {
            arguments
        };
    Ok(Some(PagerCommand { program, arguments }))
}

