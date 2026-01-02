//! External command execution utilities.
//!
//! Provides a Builder-based API for running shell commands with proper
//! output handling, PTY support, and stdin piping.

#![allow(dead_code)]
//!
//! # Examples
//!
//! ```ignore
//! use crate::utils::exec::Cmd;
//!
//! // Simple command
//! Cmd::new("git").args(["status", "-s"]).run()?;
//!
//! // With working directory and PTY
//! Cmd::new("tailwindcss")
//!     .args(["-i", "input.css", "-o", "output.css"])
//!     .cwd(root)
//!     .pty(true)
//!     .run()?;
//!
//! // With stdin piping (for magick, ffmpeg, etc.)
//! let output = Cmd::new("magick")
//!     .args(["-background", "none", "-", "png:-"])
//!     .stdin(svg_data)
//!     .run()?;
//! ```

use crate::log;
use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use regex::Regex;
use std::{
    ffi::{OsStr, OsString},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::OnceLock,
};

// ============================================================================
// Builder API
// ============================================================================

/// Command builder for external process execution.
///
/// Provides a fluent API for configuring and running external commands.
#[derive(Default)]
pub struct Cmd {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    envs: Vec<(String, String)>,
    stdin_data: Option<Vec<u8>>,
    use_pty: bool,
    filter: Option<&'static FilterRule>,
}

impl Cmd {
    /// Create a new command builder.
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            ..Default::default()
        }
    }

    /// Create from a command array (e.g., `["git"]` or `["npx", "tailwindcss"]`).
    pub fn from_slice<S: AsRef<OsStr>>(cmd: &[S]) -> Self {
        let mut iter = cmd.iter();
        let program = iter
            .next()
            .map(|s| s.as_ref().to_owned())
            .unwrap_or_default();
        let args: Vec<_> = iter.map(|s| s.as_ref().to_owned()).collect();
        Self {
            program,
            args,
            ..Default::default()
        }
    }

    /// Add a single argument.
    pub fn arg<S: AsRef<OsStr>>(mut self, arg: S) -> Self {
        let arg = arg.as_ref();
        if !arg.is_empty() {
            self.args.push(arg.to_owned());
        }
        self
    }

    /// Add multiple arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            let arg = arg.as_ref();
            if !arg.is_empty() {
                self.args.push(arg.to_owned());
            }
        }
        self
    }

    /// Set working directory.
    pub fn cwd<P: AsRef<Path>>(mut self, dir: P) -> Self {
        self.cwd = Some(dir.as_ref().to_owned());
        self
    }

    /// Set environment variables for the subprocess.
    pub fn envs<K, V, I>(mut self, vars: I) -> Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
        I: IntoIterator<Item = (K, V)>,
    {
        for (k, v) in vars {
            self.envs.push((k.as_ref().to_owned(), v.as_ref().to_owned()));
        }
        self
    }

    /// Set stdin data to pipe to the process.
    pub fn stdin<D: AsRef<[u8]>>(mut self, data: D) -> Self {
        self.stdin_data = Some(data.as_ref().to_vec());
        self
    }

    /// Enable PTY (pseudo-terminal) mode.
    ///
    /// PTY allows commands to behave as if running in a real terminal,
    /// enabling colored output, progress bars, etc.
    pub fn pty(mut self, enable: bool) -> Self {
        self.use_pty = enable;
        self
    }

    /// Set output filter for logging.
    pub fn filter(mut self, filter: &'static FilterRule) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Execute the command and return output.
    pub fn run(self) -> Result<Output> {
        let filter = self.filter.unwrap_or(&EMPTY_FILTER);

        if self.stdin_data.is_some() {
            self.run_with_stdin(filter)
        } else if self.use_pty {
            self.run_with_pty(filter)
        } else {
            self.run_simple(filter)
        }
    }
}

// ============================================================================
// Macro helper traits
// ============================================================================

/// Create a command from a single program name.
///
/// This is a helper for the `exec!` macro.
#[inline]
pub fn cmd<S: AsRef<OsStr>>(program: S) -> Cmd {
    Cmd::new(program)
}

/// Create a command from a slice of arguments.
///
/// The first element is the program, rest are arguments.
/// This is a helper for the `exec!` macro.
#[inline]
pub fn cmd_slice<S: AsRef<OsStr>>(slice: &[S]) -> Cmd {
    Cmd::from_slice(slice)
}

impl Cmd {

    /// Get the program name for error messages.
    fn program_name(&self) -> String {
        self.program.to_string_lossy().to_string()
    }

    /// Simple execution without PTY or stdin.
    fn run_simple(self, filter: &'static FilterRule) -> Result<Output> {
        let name = self.program_name();
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).envs(self.envs.iter().cloned());

        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }

        let output = cmd
            .output()
            .with_context(|| format!("Failed to execute `{name}`"))?;

        log_output(&name, &output, filter)?;
        Ok(output)
    }

    /// Execution with stdin piping.
    fn run_with_stdin(self, filter: &'static FilterRule) -> Result<Output> {
        let name = self.program_name();
        let stdin_data = self.stdin_data.unwrap();

        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .envs(self.envs.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn `{name}`"))?;

        // Write stdin data
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&stdin_data)
                .with_context(|| format!("Failed to write stdin to `{name}`"))?;
        }

        let output = child
            .wait_with_output()
            .with_context(|| format!("Failed to wait for `{name}`"))?;

        if !output.status.success() {
            anyhow::bail!(format_error(&name, &output, filter));
        }

        Ok(output)
    }

    /// Execution with PTY support.
    ///
    /// PTY allows commands to behave as if running in a real terminal,
    /// enabling colored output, progress bars, credential prompts, etc.
    fn run_with_pty(self, filter: &'static FilterRule) -> Result<Output> {
        let name = self.program_name();

        let mut cmd_builder = CommandBuilder::new(&self.program);
        cmd_builder.args(&self.args);

        // Inject environment variables
        for (k, v) in &self.envs {
            cmd_builder.env(k, v);
        }

        if let Some(dir) = &self.cwd {
            cmd_builder.cwd(dir);
        }

        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut child = pair.slave.spawn_command(cmd_builder)?;
        drop(pair.slave);

        // Read output in separate thread (PTY blocks until EOF)
        let mut reader = pair.master.try_clone_reader()?;
        let output_handle = std::thread::spawn(move || {
            let mut output = String::new();
            let _ = reader.read_to_string(&mut output);
            output
        });

        let status = child.wait()?;
        drop(pair.master);

        let output_str = output_handle
            .join()
            .map_err(|_| anyhow::anyhow!("Failed to join output reader thread"))?;

        if !status.success() {
            anyhow::bail!("Command `{name}` failed: {status:?}\n{output_str}");
        }

        filter.log(&name, &output_str);

        // Convert to std::process::Output
        #[cfg(unix)]
        #[allow(clippy::cast_possible_wrap)]
        let std_status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw((status.exit_code() as i32) << 8)
        };
        #[cfg(windows)]
        let std_status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(status.exit_code())
        };

        Ok(Output {
            status: std_status,
            stdout: output_str.into_bytes(),
            stderr: Vec::new(),
        })
    }
}

// ============================================================================
// Macro (syntax sugar for simple cases)
// ============================================================================

/// Run an external command with arguments.
///
/// # Syntax
///
/// ```ignore
/// // Simple: command and args
/// exec!("git"; "status", "-s")?;
/// exec!(["git"]; "status", "-s")?;
///
/// // With working directory
/// exec!(root; "git"; "status")?;
/// exec!(root; ["git"]; "status")?;
///
/// // With options (pty, filter)
/// exec!(pty=true; root; ["git"]; "push")?;
/// exec!(pty=true; filter=&F; root; &cmd; args...)?;
/// ```
#[macro_export]
macro_rules! exec {
    // pty + filter + root + array cmd
    (pty=$pty:expr; filter=$filter:expr; $root:expr; [$($cmd:expr),+ $(,)?]; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd_slice(&[$($cmd),+])
            $(.arg($arg))*
            .cwd($root)
            .pty($pty)
            .filter($filter)
            .run()
    };
    // pty + filter + root + single cmd
    (pty=$pty:expr; filter=$filter:expr; $root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd($cmd)
            $(.arg($arg))*
            .cwd($root)
            .pty($pty)
            .filter($filter)
            .run()
    };

    // pty + root + array cmd
    (pty=$pty:expr; $root:expr; [$($cmd:expr),+ $(,)?]; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd_slice(&[$($cmd),+])
            $(.arg($arg))*
            .cwd($root)
            .pty($pty)
            .run()
    };
    // pty + root + single cmd
    (pty=$pty:expr; $root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd($cmd)
            $(.arg($arg))*
            .cwd($root)
            .pty($pty)
            .run()
    };

    // root + array cmd
    ($root:expr; [$($cmd:expr),+ $(,)?]; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd_slice(&[$($cmd),+])
            $(.arg($arg))*
            .cwd($root)
            .run()
    };
    // root + single cmd
    ($root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd($cmd)
            $(.arg($arg))*
            .cwd($root)
            .run()
    };

    // array cmd only
    ([$($cmd:expr),+ $(,)?]; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd_slice(&[$($cmd),+])
            $(.arg($arg))*
            .run()
    };
    // single cmd only
    ($cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::cmd($cmd)
            $(.arg($arg))*
            .run()
    };
}

// ============================================================================
// Output Filtering
// ============================================================================

/// Filter rule for command output logging.
///
/// Used to reduce noise by skipping known warnings or irrelevant messages.
pub struct FilterRule {
    /// Prefixes to skip when logging output.
    pub skip_prefixes: &'static [&'static str],
}

impl FilterRule {
    /// Create a new filter rule.
    pub const fn new(skip_prefixes: &'static [&'static str]) -> Self {
        Self { skip_prefixes }
    }

    /// Check if a line should be skipped.
    fn should_skip(&self, line: &str) -> bool {
        line.is_empty() || self.skip_prefixes.iter().any(|p| line.starts_with(p))
    }

    /// Log output lines that pass the filter.
    pub fn log(&self, name: &str, output: &str) {
        let lines: Vec<_> = output
            .lines()
            .filter(|line| {
                let plain = strip_ansi(line);
                let trimmed = plain.trim();
                !trimmed.is_empty() && !self.should_skip(trimmed)
            })
            .collect();

        if !lines.is_empty() {
            log!(name; "{}", lines.join("\n"));
        }
    }
}

/// Empty filter (no skipping).
pub const EMPTY_FILTER: FilterRule = FilterRule::new(&[]);

/// Silent filter (skip all output).
pub const SILENT_FILTER: FilterRule = FilterRule::new(&[""]);

// ============================================================================
// Helpers
// ============================================================================

/// Strip ANSI escape codes from string.
fn strip_ansi(s: &str) -> std::borrow::Cow<'_, str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    re.replace_all(s, "")
}

/// Log command output, returning error on failure.
fn log_output(name: &str, output: &Output, filter: &'static FilterRule) -> Result<()> {
    if !output.status.success() {
        anyhow::bail!(format_error(name, output, filter));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    filter.log(name, stderr.trim());
    Ok(())
}

/// Format error message for failed command.
fn format_error(name: &str, output: &Output, filter: &'static FilterRule) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    let error_msg = filter
        .skip_prefixes
        .iter()
        .fold(stderr.trim(), |s, p| s.trim_start_matches(p).trim_start());

    let mut msg = format!("Command `{name}` failed with {}\n", output.status);
    if !error_msg.is_empty() {
        msg.push_str(error_msg);
    }

    let stdout_trimmed = stdout.trim();
    if !stdout_trimmed.is_empty()
        && !stdout_trimmed.starts_with("<!DOCTYPE")
        && !stdout_trimmed.starts_with('{')
    {
        msg.push_str("\nStdout:\n");
        msg.push_str(stdout_trimmed);
    }
    msg
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmd_builder() {
        let cmd = Cmd::new("echo")
            .arg("hello")
            .args(["world", "!"])
            .cwd("/tmp");

        assert_eq!(cmd.program, OsString::from("echo"));
        assert_eq!(cmd.args.len(), 3);
        assert_eq!(cmd.cwd, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn test_empty_args_filtered() {
        let cmd = Cmd::new("echo").arg("").args(["a", "", "b"]);
        assert_eq!(cmd.args.len(), 2);
    }

    #[test]
    fn test_filter_rule() {
        let filter = FilterRule::new(&["WARN:", "INFO:"]);
        assert!(filter.should_skip("WARN: something"));
        assert!(filter.should_skip("INFO: something"));
        assert!(!filter.should_skip("ERROR: something"));
        assert!(filter.should_skip(""));
    }

    #[test]
    fn test_strip_ansi() {
        assert_eq!(strip_ansi("\x1b[31mRed\x1b[0m"), "Red");
        assert_eq!(strip_ansi("Plain text"), "Plain text");
    }

    #[test]
    fn test_simple_command() {
        let output = Cmd::new("echo").arg("hello").run().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    #[test]
    fn test_stdin_pipe() {
        let output = Cmd::new("cat").stdin(b"test data").run().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"test data");
    }
}
