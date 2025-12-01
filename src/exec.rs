//! External command execution utilities.
//!
//! Provides macros and functions for running shell commands with proper
//! output handling and error reporting.

use crate::log;
use anyhow::{Context, Result};
use std::{
    ffi::OsString,
    path::Path,
    process::{Child, ChildStdin, Command, Output, Stdio},
};

// ============================================================================
// Macros
// ============================================================================

/// Run an external command with arguments.
///
/// # Examples
/// ```ignore
/// // Without working directory
/// exec!(["git"]; "status", "-s")?;
///
/// // With working directory
/// exec!(root; ["typst"]; "compile", input, output)?;
/// ```
#[macro_export]
macro_rules! exec {
    ($cmd:expr; $($arg:expr),* $(,)?) => {{
        $crate::exec::exec(
            None,
            &$crate::exec::to_cmd_vec($cmd),
            &$crate::exec::filter_args(&[$($crate::exec::to_os($arg)),*]),
        )
    }};
    ($root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {{
        $crate::exec::exec(
            Some($root),
            &$crate::exec::to_cmd_vec($cmd),
            &$crate::exec::filter_args(&[$($crate::exec::to_os($arg)),*]),
        )
    }};
}

/// Run an external command and return a handle to its stdin.
///
/// The child process is spawned with stdin piped, stdout/stderr nulled.
/// Caller is responsible for writing to stdin and dropping it to signal EOF.
#[macro_export]
macro_rules! exec_with_stdin {
    ($cmd:expr; $($arg:expr),* $(,)?) => {{
        $crate::exec::spawn_with_stdin(
            None,
            &$crate::exec::to_cmd_vec($cmd),
            &$crate::exec::filter_args(&[$($crate::exec::to_os($arg)),*]),
        )
    }};
    ($root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {{
        $crate::exec::spawn_with_stdin(
            Some($root),
            &$crate::exec::to_cmd_vec($cmd),
            &$crate::exec::filter_args(&[$($crate::exec::to_os($arg)),*]),
        )
    }};
}

// ============================================================================
// Argument Conversion
// ============================================================================

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

// ============================================================================
// Command Execution
// ============================================================================

/// Execute a command and capture its output.
///
/// # Errors
/// Returns error if command fails to execute or returns non-zero exit code.
pub fn exec(root: Option<&Path>, cmd: &[OsString], args: &[OsString]) -> Result<Output> {
    let (name, mut command) = prepare(root, cmd, args)?;

    let output = command
        .output()
        .with_context(|| format!("Failed to execute `{name}`"))?;

    log_output(&name, &output)?;
    Ok(output)
}

/// Spawn a command and return handles to its stdin and the child process.
///
/// The caller should:
/// 1. Write data to the returned `ChildStdin`
/// 2. Pass both `ChildStdin` and `Child` to [`wait_child`] to close stdin and wait for completion
///
/// # Errors
/// Returns error if command fails to spawn.
pub fn spawn_with_stdin(
    root: Option<&Path>,
    cmd: &[OsString],
    args: &[OsString],
) -> Result<(ChildStdin, Child)> {
    let (name, mut command) = prepare(root, cmd, args)?;

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child: Child = command
        .spawn()
        .with_context(|| format!("Failed to spawn `{name}`"))?;

    let stdin = child.stdin.take().context("Failed to acquire stdin")?;
    Ok((stdin, child))
}

/// Wait for child process to complete and check exit status.
///
/// Takes ownership of `stdin` to ensure the pipe is closed before waiting.
/// This is critical: the child process reads from stdin until EOF, so we must
/// close the pipe (via `drop`) before `wait()`, otherwise it will deadlock.
///
/// Note: `drop(stdin)` must be explicit here. Using `_stdin` parameter name or
/// `let _ = stdin` would defer the drop until function end, after `wait()`.
///
/// # Errors
/// Returns error if process exits with non-zero status.
pub fn wait_child(stdin: ChildStdin, mut child: Child, name: &str) -> Result<()> {
    drop(stdin); // Must close stdin before wait, otherwise child blocks on read
    let status = child.wait().context(format!("{name} process failed"))?;
    if !status.success() {
        let stderr = child
            .stderr
            .take()
            .map(|s| std::io::read_to_string(s).unwrap_or_default())
            .unwrap_or_default();
        anyhow::bail!("{name} failed with {status}: {stderr}");
    }
    Ok(())
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

// ============================================================================
// Output Filtering
// ============================================================================

/// Filter rule for skipping entire output blocks.
///
/// If output starts with any of the specified prefixes, the entire block is skipped.
struct FilterRule {
    /// Prefixes to match at the start of output.
    skip_prefixes: &'static [&'static str],
}

impl FilterRule {
    const fn new(skip_prefixes: &'static [&'static str]) -> Self {
        Self { skip_prefixes }
    }

    /// Check if output should be skipped entirely.
    fn should_skip(&self, output: &str) -> bool {
        output.is_empty() || self.skip_prefixes.iter().any(|p| output.starts_with(p))
    }

    /// Log output lines if not skipped.
    fn log(&self, name: &str, output: &str) {
        if self.should_skip(output) {
            return;
        }
        for line in output.lines() {
            if !line.trim().is_empty() {
                log!(name; "{line}");
            }
        }
    }
}

/// Stdout filter: skip HTML and JSON output.
const STDOUT_FILTER: FilterRule = FilterRule::new(&["<!DOCTYPE", "{"]);

/// Stderr filter: skip known warnings/noise.
const STDERR_FILTER: FilterRule = FilterRule::new(&[
    // Typst HTML export warnings (multi-line)
    "warning: html export is under active development",
    "and incomplete",
    "= hint: its behaviour may change at any time",
    "= hint: do not rely on this feature for production use cases",
    "= hint: see https://github.com/typst/typst/issues/5512",
    "for more information",
    "warning: elem",
    // Tailwindcss version banner
    "≈ tailwindcss",
]);

/// Log command output, filtering known noise.
fn log_output(name: &str, output: &Output) -> Result<()> {
    let stdout = std::str::from_utf8(&output.stdout)
        .context("Invalid UTF-8 in stdout")?
        .trim();
    let stderr = std::str::from_utf8(&output.stderr)
        .context("Invalid UTF-8 in stderr")?
        .trim();

    if !output.status.success() {
        // Strip warning prefix from error output
        let error_msg = STDERR_FILTER
            .skip_prefixes
            .iter()
            .fold(stderr, |s, p| s.trim_start_matches(p).trim_start());

        let mut msg = format!("Command `{name}` failed with {}\n", output.status);
        if !error_msg.is_empty() {
            msg.push_str("\nStderr:\n");
            msg.push_str(error_msg);
        }
        if !stdout.is_empty() && !STDOUT_FILTER.should_skip(stdout) {
            msg.push_str("\nStdout:\n");
            msg.push_str(stdout);
        }
        anyhow::bail!(msg);
    }

    // On success, only log stderr (warnings) to reduce noise
    // STDOUT_FILTER.log(name, stdout);
    STDERR_FILTER.log(name, stderr);

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
}
