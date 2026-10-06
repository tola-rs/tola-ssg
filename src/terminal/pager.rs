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
    pub(crate) fn for_process() -> Option<Self> {
        if !io::stdout().is_terminal() {
            return None;
        }
        let term = std::env::var_os("TERM");
        if term.is_none() || term.as_deref() == Some(OsStr::new("dumb")) {
            return None;
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
/// file stem is `cat`, disables paging instead of falling through to the next source. A value
/// splits on whitespace with no shell quoting, the way `systemd` and `man` read theirs.
fn resolve(
    tola_pager: Option<&OsStr>,
    pager: Option<&OsStr>,
    less: Option<&Path>,
) -> Option<PagerCommand> {
    let (program, arguments): (OsString, Vec<OsString>) = match tola_pager.or(pager) {
        Some(configured) => {
            let configured = configured.to_string_lossy();
            let mut fields = configured.split_whitespace();
            let program = fields.next()?;
            if Path::new(program).file_stem() == Some(OsStr::new("cat")) {
                return None;
            }
            (program.into(), fields.map(OsString::from).collect())
        }
        None => (less?.as_os_str().to_os_string(), Vec::new()),
    };
    let arguments =
        if arguments.is_empty() && Path::new(&program).file_stem() == Some(OsStr::new("less")) {
            LESS_ARGUMENTS.iter().copied().map(OsString::from).collect()
        } else {
            arguments
        };
    Some(PagerCommand { program, arguments })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{OutputSink, Terminal};

    fn resolved(program: &str, arguments: &[&str]) -> Option<PagerCommand> {
        Some(PagerCommand {
            program: program.into(),
            arguments: arguments.iter().copied().map(OsString::from).collect(),
        })
    }

    #[test]
    fn tola_pager_decides_before_pager() {
        assert_eq!(
            resolve(
                Some(OsStr::new("more")),
                Some(OsStr::new("less -S")),
                Some(Path::new("/usr/bin/less")),
            ),
            resolved("more", &[])
        );
    }

    #[test]
    fn empty_or_cat_variable_disables_paging() {
        let less = Some(Path::new("/usr/bin/less"));
        for value in ["", "  ", "cat", "/bin/cat", "cat -u"] {
            let configured = Some(OsStr::new(value));
            assert_eq!(
                resolve(configured, None, less),
                None,
                "TOLA_PAGER={value:?}"
            );
            assert_eq!(resolve(None, configured, less), None, "PAGER={value:?}");
        }
    }

    #[test]
    fn bare_less_receives_documentation_arguments() {
        assert_eq!(
            resolve(Some(OsStr::new("less")), None, None),
            resolved("less", LESS_ARGUMENTS)
        );
    }

    #[test]
    fn written_less_arguments_are_kept() {
        assert_eq!(
            resolve(Some(OsStr::new("less -S")), None, None),
            resolved("less", &["-S"])
        );
    }

    #[test]
    fn unset_variables_use_path_less() {
        assert_eq!(
            resolve(None, None, Some(Path::new("/usr/bin/less"))),
            resolved("/usr/bin/less", LESS_ARGUMENTS)
        );
    }

    #[test]
    fn missing_pager_leaves_stdout_unpaged() {
        assert_eq!(resolve(None, None, None), None);
    }

    #[cfg(unix)]
    #[test]
    fn started_pager_receives_documentation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documentation.txt");
        let pager = PagerCommand {
            program: "sh".into(),
            arguments: vec!["-c".into(), format!("cat > \"{}\"", path.display()).into()],
        };
        pager
            .start()
            .unwrap()
            .write(b"documentation bytes")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "documentation bytes"
        );
    }

    #[test]
    fn missing_pager_falls_back_to_terminal() {
        let (sink, output) = OutputSink::buffered();
        let mut terminal = Terminal::with_sink(sink, false, false);
        terminal.pager = Some(PagerCommand {
            program: "tola-pager-that-does-not-exist".into(),
            arguments: Vec::new(),
        });
        terminal.write_documentation(b"documentation").unwrap();

        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(
            shown.contains("tola-pager-that-does-not-exist"),
            "{shown:?}"
        );
        assert!(shown.ends_with("documentation"), "{shown:?}");
    }
}
