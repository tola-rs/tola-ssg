//! External command execution utilities.
//!
//! Provides macros and functions for running shell commands with proper
//! output handling and error reporting.

use crate::log;
use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use regex::Regex;
use std::{
    ffi::OsString,
    io::Read,
    path::Path,
    process::{Child, ChildStdin, Command, Output, Stdio},
    sync::OnceLock,
};

// ============================================================================
// Macros
// ============================================================================

/// Run an external command with arguments.
///
/// Supports optional `pty` and `filter` arguments.
///
/// # Examples
/// ```ignore
/// // Without working directory
/// exec!(["git"]; "status", "-s")?;
///
/// // With working directory
/// exec!(root; ["typst"]; "compile", input, output)?;
///
/// // With custom filter
/// const MY_FILTER: FilterRule = FilterRule::new(&["warning:"]);
/// exec!(filter=&MY_FILTER; ["typst"]; "compile")?;
///
/// // With PTY enabled
/// exec!(pty=true; ["typst"]; "compile")?;
/// ```
#[macro_export]
macro_rules! exec {
    ($($tt:tt)*) => {
        $crate::exec_internal!(@parse_pty $($tt)*)
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! exec_internal {
    // Parse pty argument
    (@parse_pty pty=$pty:expr; $($rest:tt)*) => {
        $crate::exec_internal!(@parse_filter $pty; $($rest)*)
    };
    (@parse_pty $($rest:tt)*) => {
        $crate::exec_internal!(@parse_filter false; $($rest)*)
    };

    // Parse filter argument
    (@parse_filter $pty:expr; filter=$filter:expr; $($rest:tt)*) => {
        $crate::exec_internal!(@parse_root $pty; $filter; $($rest)*)
    };
    (@parse_filter $pty:expr; $($rest:tt)*) => {
        $crate::exec_internal!(@parse_root $pty; &$crate::utils::exec::EMPTY_FILTER; $($rest)*)
    };

    // Parse root and command (with root)
    (@parse_root $pty:expr; $filter:expr; $root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::exec(
            Some($root),
            &$crate::utils::exec::internal::to_cmd_vec($cmd),
            &$crate::utils::exec::internal::filter_args(&[$($crate::utils::exec::internal::to_os($arg)),*]),
            $filter,
            $pty,
        )
    };
    // Parse command (without root)
    (@parse_root $pty:expr; $filter:expr; $cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::exec(
            None,
            &$crate::utils::exec::internal::to_cmd_vec($cmd),
            &$crate::utils::exec::internal::filter_args(&[$($crate::utils::exec::internal::to_os($arg)),*]),
            $filter,
            $pty,
        )
    };
}

/// Run an external command and return a `RunningProcess` handle.
///
/// The child process is spawned with stdin piped, stdout/stderr nulled.
/// Caller must write to stdin via `proc.stdin()` and call `proc.wait()` to finish.
///
/// # Examples
/// ```ignore
/// let mut proc = exec_with_stdin!(["cat"];)?;
/// if let Some(stdin) = proc.stdin() {
///     stdin.write_all(b"hello")?;
/// }
/// proc.wait()?;
/// ```
#[macro_export]
macro_rules! exec_with_stdin {
    ($($tt:tt)*) => {
        $crate::exec_with_stdin_internal!(@parse_pty $($tt)*)
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! exec_with_stdin_internal {
    // Parse pty argument
    (@parse_pty pty=$pty:expr; $($rest:tt)*) => {
        $crate::exec_with_stdin_internal!(@parse_filter $pty; $($rest)*)
    };
    (@parse_pty $($rest:tt)*) => {
        $crate::exec_with_stdin_internal!(@parse_filter false; $($rest)*)
    };

    // Parse filter argument
    (@parse_filter $pty:expr; filter=$filter:expr; $($rest:tt)*) => {
        $crate::exec_with_stdin_internal!(@parse_root $pty; $filter; $($rest)*)
    };
    (@parse_filter $pty:expr; $($rest:tt)*) => {
        $crate::exec_with_stdin_internal!(@parse_root $pty; &$crate::utils::exec::EMPTY_FILTER; $($rest)*)
    };

    // Parse root and command (with root)
    (@parse_root $pty:expr; $filter:expr; $root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::spawn_with_stdin(
            Some($root),
            &$crate::utils::exec::internal::to_cmd_vec($cmd),
            &$crate::utils::exec::internal::filter_args(&[$($crate::utils::exec::internal::to_os($arg)),*]),
            $filter,
            $pty,
        )
    };
    // Parse command (without root)
    (@parse_root $pty:expr; $filter:expr; $cmd:expr; $($arg:expr),* $(,)?) => {
        $crate::utils::exec::spawn_with_stdin(
            None,
            &$crate::utils::exec::internal::to_cmd_vec($cmd),
            &$crate::utils::exec::internal::filter_args(&[$($crate::utils::exec::internal::to_os($arg)),*]),
            $filter,
            $pty,
        )
    };
}

// ============================================================================
// Argument Conversion
// ============================================================================

#[doc(hidden)]
pub mod internal {
    use super::*;

    /// Convert to OsString.
    #[inline]
    pub fn to_os<S: Into<OsString>>(s: S) -> OsString {
        s.into()
    }

    /// Trait for converting to command vector.
    pub trait ToCmd {
        fn to_cmd(self) -> Vec<OsString>;
    }

    impl<const N: usize> ToCmd for [&str; N] {
        #[inline]
        fn to_cmd(self) -> Vec<OsString> {
            self.into_iter().map(OsString::from).collect()
        }
    }

    impl ToCmd for &[String] {
        #[inline]
        fn to_cmd(self) -> Vec<OsString> {
            self.iter().map(OsString::from).collect()
        }
    }

    impl ToCmd for &Vec<String> {
        #[inline]
        fn to_cmd(self) -> Vec<OsString> {
            self.iter().map(OsString::from).collect()
        }
    }

    /// Convert command to Vec<OsString>.
    #[inline]
    pub fn to_cmd_vec<C: ToCmd>(cmd: C) -> Vec<OsString> {
        cmd.to_cmd()
    }

    /// Filter out empty args.
    #[inline]
    pub fn filter_args(args: &[OsString]) -> Vec<OsString> {
        args.iter().filter(|a| !a.is_empty()).cloned().collect()
    }
}

// ============================================================================
// Command Execution
// ============================================================================

/// Execute a command and capture its output.
///
/// # Errors
/// Returns error if command fails to execute or returns non-zero exit code.
pub fn exec(
    root: Option<&Path>,
    cmd: &[OsString],
    args: &[OsString],
    filter: &'static FilterRule,
    pty: bool,
) -> Result<Output> {
    if pty {
        exec_with_pty(root, cmd, args, filter)
    } else {
        exec_no_pty(root, cmd, args, filter)
    }
}

fn exec_no_pty(
    root: Option<&Path>,
    cmd: &[OsString],
    args: &[OsString],
    filter: &'static FilterRule,
) -> Result<Output> {
    let (name, mut command) = prepare(root, cmd, args)?;

    let output = command
        .output()
        .with_context(|| format!("Failed to execute `{name}`"))?;

    log_output(&name, &output, filter)?;
    Ok(output)
}

/// Execute a command with PTY (pseudo-terminal) support.
///
/// PTY allows commands to behave as if running in a real terminal, which is
/// necessary for programs that check `isatty()` or need terminal features
/// (colored output, progress bars, credential prompts, etc.).
///
/// # PTY Architecture
///
/// A PTY consists of two ends:
/// - **Master**: The controlling side (our program). Writes become the child's
///   stdin; reads receive the child's stdout/stderr.
/// - **Slave**: The terminal side (child process). The child reads/writes to
///   this as if it were a real terminal.
///
/// ```text
///   ┌─────────────┐                    ┌─────────────┐
///   │   Master    │◄────── PTY ───────►│    Slave    │
///   │ (our code)  │                    │  (child)    │
///   │             │  write ──────────► │  stdin      │
///   │             │  read  ◄────────── │  stdout     │
///   └─────────────┘                    └─────────────┘
/// ```
///
/// # Execution Flow
///
/// 1. Open PTY pair (master + slave)
/// 2. Spawn child process attached to slave
/// 3. Drop slave (we don't need it; child has its own handle)
/// 4. Spawn reader thread to collect output from master
/// 5. Wait for child to exit
/// 6. Drop master to signal EOF to reader thread
/// 7. Join reader thread to get collected output
///
/// The reader thread is necessary because `read_to_string()` blocks until EOF,
/// and EOF only occurs when the master is dropped. Without threading, we'd
/// deadlock: waiting for read to finish before dropping master, but read waits
/// for master to drop.
fn exec_with_pty(
    root: Option<&Path>,
    cmd: &[OsString],
    args: &[OsString],
    filter: &'static FilterRule,
) -> Result<Output> {
    let (name, command_builder) = prepare_pty(root, cmd, args)?;

    let pty_system = NativePtySystem::default();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    // Spawn child process attached to the slave end of PTY.
    // The child sees the slave as its controlling terminal.
    let mut child = pair.slave.spawn_command(command_builder)?;

    // Drop our handle to slave. The child process has its own handle,
    // so the slave remains open for the child to use.
    drop(pair.slave);

    // Clone a reader from master to read child's output.
    // This must run in a separate thread because `read_to_string()` blocks
    // until EOF, which only happens when we drop the master.
    let mut reader = pair.master.try_clone_reader()?;
    let output_handle = std::thread::spawn(move || {
        let mut output_str = String::new();
        let _ = reader.read_to_string(&mut output_str);
        output_str
    });

    // Wait for child process to complete.
    // The child may still be writing output at this point.
    let status = child.wait()?;

    // Drop master to close the PTY. This signals EOF to the reader thread,
    // allowing `read_to_string()` to return.
    drop(pair.master);

    // Join the reader thread to collect all output.
    let output_str = output_handle
        .join()
        .map_err(|_| anyhow::anyhow!("Failed to join output reader thread"))?;

    if !status.success() {
        let msg = format!(
            "Command `{name}` failed with exit code: {:?}\n{}",
            status, output_str
        );
        anyhow::bail!(msg);
    }

    filter.log(&name, &output_str);

    // Convert portable_pty::ExitStatus to std::process::ExitStatus
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        let code = status.exit_code() as i32;
        std::process::ExitStatus::from_raw(code << 8)
    };
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        let code = status.exit_code();
        std::process::ExitStatus::from_raw(code)
    };

    Ok(Output {
        status,
        stdout: output_str.into_bytes(),
        stderr: Vec::new(),
    })
}

/// Spawn a command and return a `RunningProcess` handle.
///
/// # Errors
/// Returns error if command fails to spawn.
pub fn spawn_with_stdin(
    root: Option<&Path>,
    cmd: &[OsString],
    args: &[OsString],
    filter: &'static FilterRule,
    pty: bool,
) -> Result<RunningProcess> {
    if pty {
        anyhow::bail!("PTY not supported for spawn_with_stdin yet");
    }

    let (name, mut command) = prepare(root, cmd, args)?;

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child: Child = command
        .spawn()
        .with_context(|| format!("Failed to spawn `{name}`"))?;

    let stdin = child.stdin.take().context("Failed to acquire stdin")?;
    Ok(RunningProcess {
        child,
        stdin: Some(stdin),
        name,
        filter,
    })
}

/// A running child process with piped stdin.
///
/// Encapsulates the lifecycle of a process that expects input via stdin.
/// Ensures stdin is closed before waiting for the process to exit.
pub struct RunningProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    name: String,
    filter: &'static FilterRule,
}

impl RunningProcess {
    /// Get a mutable reference to the child's stdin.
    pub fn stdin(&mut self) -> Option<&mut ChildStdin> {
        self.stdin.as_mut()
    }

    /// Wait for child process to complete and check exit status.
    ///
    /// Automatically closes stdin to signal EOF to the child process.
    ///
    /// # Errors
    /// Returns error if process exits with non-zero status.
    pub fn wait(mut self) -> Result<()> {
        // Must close stdin before wait, otherwise child blocks on read
        drop(self.stdin.take());

        let status = self
            .child
            .wait()
            .context(format!("{} process failed", self.name))?;

        if !status.success() {
            let mut stderr_bytes = Vec::new();
            if let Some(mut stderr) = self.child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_end(&mut stderr_bytes);
            }

            let output = Output {
                status,
                stdout: Vec::new(),
                stderr: stderr_bytes,
            };

            anyhow::bail!(format_error(&self.name, &output, self.filter));
        }
        Ok(())
    }
}

/// Prepare a Command from components.
fn prepare(root: Option<&Path>, cmd: &[OsString], args: &[OsString]) -> Result<(String, Command)> {
    let name = cmd
        .first()
        .and_then(|s| s.to_str())
        .context("Empty command")?
        .to_owned();

    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]).args(args);

    if let Some(dir) = root {
        command.current_dir(dir);
    }

    Ok((name, command))
}

/// Prepare a CommandBuilder from components.
fn prepare_pty(
    root: Option<&Path>,
    cmd: &[OsString],
    args: &[OsString],
) -> Result<(String, CommandBuilder)> {
    let name = cmd
        .first()
        .and_then(|s| s.to_str())
        .context("Empty command")?
        .to_owned();

    let mut command = CommandBuilder::new(&cmd[0]);

    let mut all_args = Vec::new();
    all_args.extend_from_slice(&cmd[1..]);
    all_args.extend_from_slice(args);

    command.args(&all_args);

    if let Some(dir) = root {
        command.cwd(dir);
    }

    Ok((name, command))
}

// ============================================================================
// Output Filtering
// ============================================================================

fn strip_ansi(s: &str) -> std::borrow::Cow<'_, str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    re.replace_all(s, "")
}

/// Filter rule for skipping entire output blocks or specific prefixes.
///
/// Used to reduce noise in command output logging by ignoring known warnings
/// or irrelevant messages.
pub struct FilterRule {
    /// Prefixes to match at the start of output lines.
    pub skip_prefixes: &'static [&'static str],
}

impl FilterRule {
    /// Create a new filter rule with the given prefixes.
    pub const fn new(skip_prefixes: &'static [&'static str]) -> Self {
        Self { skip_prefixes }
    }

    /// Check if output should be skipped entirely.
    ///
    /// Returns true if output is empty or starts with any of the skip prefixes.
    fn should_skip(&self, output: &str) -> bool {
        output.is_empty() || self.skip_prefixes.iter().any(|p| output.starts_with(p))
    }

    /// Log output lines if not skipped.
    ///
    /// Iterates through lines and logs them using the `log!` macro if they
    /// don't match the skip criteria.
    fn log(&self, name: &str, output: &str) {
        let mut valid_lines = Vec::new();
        for line in output.lines() {
            let plain = strip_ansi(line);
            let trimmed = plain.trim();
            if !trimmed.is_empty() && !self.should_skip(trimmed) {
                valid_lines.push(line);
            }
        }

        if !valid_lines.is_empty() {
            let message = valid_lines.join("\n");
            log!(name; "{}", message);
        }
    }
}

/// Stdout filter: skip HTML and JSON output.
const STDOUT_FILTER: FilterRule = FilterRule::new(&["<!DOCTYPE", "{"]);

/// Empty filter (no skipping).
pub const EMPTY_FILTER: FilterRule = FilterRule::new(&[]);

/// Silent filter: skip all output.
pub const SILENT_FILTER: FilterRule = FilterRule::new(&[""]);

/// Log command output, filtering known noise.
fn log_output(name: &str, output: &Output, filter: &'static FilterRule) -> Result<()> {
    if !output.status.success() {
        anyhow::bail!(format_error(name, output, filter));
    }

    // On success, only log stderr (warnings) to reduce noise
    let stderr = String::from_utf8_lossy(&output.stderr);
    filter.log(name, stderr.trim());

    Ok(())
}

/// Format command error message with filtering.
fn format_error(name: &str, output: &Output, filter: &'static FilterRule) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Strip warning prefix from error output
    let error_msg = filter
        .skip_prefixes
        .iter()
        .fold(stderr.trim(), |s, p| s.trim_start_matches(p).trim_start());

    let mut msg = format!("Command `{name}` failed with {}\n", output.status);
    if !error_msg.is_empty() {
        msg.push_str(error_msg);
    }

    let stdout_trimmed = stdout.trim();
    if !stdout_trimmed.is_empty() && !STDOUT_FILTER.should_skip(stdout_trimmed) {
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
    use crate::utils::exec::internal::*;

    #[test]
    fn test_to_os() {
        assert_eq!(to_os("hello"), OsString::from("hello"));
        assert_eq!(to_os(String::from("world")), OsString::from("world"));
    }

    #[test]
    fn test_to_cmd_vec_array() {
        let cmd = to_cmd_vec(["git", "status"]);
        assert_eq!(cmd.len(), 2);
        assert_eq!(cmd[0], OsString::from("git"));
        assert_eq!(cmd[1], OsString::from("status"));
    }

    #[test]
    fn test_to_cmd_vec_vec() {
        let v = vec!["echo".to_string(), "hello".to_string()];
        let cmd = to_cmd_vec(&v);
        assert_eq!(cmd.len(), 2);
        assert_eq!(cmd[0], OsString::from("echo"));
        assert_eq!(cmd[1], OsString::from("hello"));
    }

    #[test]
    fn test_filter_args() {
        let args = [OsString::from("a"), OsString::from(""), OsString::from("b")];
        let filtered = filter_args(&args);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0], OsString::from("a"));
        assert_eq!(filtered[1], OsString::from("b"));
    }

    #[test]
    fn test_prepare_empty() {
        let result = prepare(None, &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_prepare_valid() {
        let cmd = to_cmd_vec(["echo"]);
        let args = filter_args(&[OsString::from("hello")]);
        let result = prepare(None, &cmd, &args);
        assert!(result.is_ok());
        let (name, _) = result.unwrap();
        assert_eq!(name, "echo");
    }

    #[test]
    fn test_filter_rule() {
        let filter = FilterRule::new(&["WARN:", "INFO:"]);

        // Test should_skip
        assert!(filter.should_skip("WARN: something"));
        assert!(filter.should_skip("INFO: something"));
        assert!(!filter.should_skip("ERROR: something"));
        assert!(filter.should_skip("")); // Empty lines skipped
    }

    #[test]
    fn test_format_error() {
        // Get a real ExitStatus using a dummy command
        // We use `ls` (unix) or `dir` (windows) which should succeed,
        // but we manually check the formatting logic which doesn't depend on success/failure
        // of the command itself, just the status object.
        // Actually, to test failure formatting, we want a failed status.
        // `false` command returns exit code 1.
        let status = Command::new("false")
            .status()
            .or_else(|_| Command::new("cmd").args(&["/C", "exit 1"]).status()) // Windows fallback
            .unwrap();

        static TEST_FILTER: FilterRule = FilterRule::new(&["Ignored:"]);
        let output = Output {
            status,
            stdout: Vec::new(),
            stderr: b"Ignored: warning\nFatal error".to_vec(),
        };
        let msg = format_error("test", &output, &TEST_FILTER);

        assert!(msg.contains("Command `test` failed"));
    }

    #[test]
    fn test_strip_ansi() {
        // Basic colors
        assert_eq!(strip_ansi("\x1b[31mRed\x1b[0m"), "Red");
        assert_eq!(strip_ansi("\x1b[1;32mGreen Bold\x1b[0m"), "Green Bold");

        // Multiple codes
        assert_eq!(strip_ansi("\x1b[31;42mRed on Green\x1b[0m"), "Red on Green");

        // No colors
        assert_eq!(strip_ansi("Plain text"), "Plain text");

        // Mixed content
        assert_eq!(
            strip_ansi("Start \x1b[33mYellow\x1b[0m End"),
            "Start Yellow End"
        );
    }
}
