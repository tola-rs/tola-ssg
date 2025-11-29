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
        $crate::utils::command::exec(
            None,
            &$crate::utils::command::to_cmd_vec($cmd),
            &$crate::utils::command::filter_args(&[$($crate::utils::command::to_os($arg)),*]),
        )
    }};
    ($root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {{
        $crate::utils::command::exec(
            Some($root),
            &$crate::utils::command::to_cmd_vec($cmd),
            &$crate::utils::command::filter_args(&[$($crate::utils::command::to_os($arg)),*]),
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
        $crate::utils::command::spawn_with_stdin(
            None,
            &$crate::utils::command::to_cmd_vec($cmd),
            &$crate::utils::command::filter_args(&[$($crate::utils::command::to_os($arg)),*]),
        )
    }};
    ($root:expr; $cmd:expr; $($arg:expr),* $(,)?) => {{
        $crate::utils::command::spawn_with_stdin(
            Some($root),
            &$crate::utils::command::to_cmd_vec($cmd),
            &$crate::utils::command::filter_args(&[$($crate::utils::command::to_os($arg)),*]),
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

/// Spawn a command and return a handle to write to its stdin.
///
/// # Errors
/// Returns error if command fails to spawn.
pub fn spawn_with_stdin(
    root: Option<&Path>,
    cmd: &[OsString],
    args: &[OsString],
) -> Result<ChildStdin> {
    let (name, mut command) = prepare(root, cmd, args)?;

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child: Child = command
        .spawn()
        .with_context(|| format!("Failed to spawn `{name}`"))?;

    child.stdin.take().context("Failed to acquire stdin")
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
// Output Logging
// ============================================================================

/// Output filter configuration.
struct OutputFilter {
    /// Prefixes to skip entirely (e.g., HTML, JSON output).
    skip_prefixes: &'static [&'static str],
    /// Warning text to strip from error messages.
    strip_warning: &'static str,
}

impl OutputFilter {
    const STDOUT: Self = Self {
        skip_prefixes: &["<!DOCTYPE html>", "{"],
        strip_warning: "",
    };

    const STDERR: Self = Self {
        skip_prefixes: &[
            "warning: html export is under active development",
            "warning: elem was ignored during paged export",
            "≈ tailwindcss v",
        ],
        strip_warning: "warning: html export is under active development and incomplete\n \
             = hint: its behaviour may change at any time\n \
             = hint: do not rely on this feature for production use cases\n \
             = hint: see https://github.com/typst/typst/issues/5512 for more information\n",
    };

    /// Check if output should be skipped.
    #[inline]
    fn should_skip(&self, output: &str) -> bool {
        output.is_empty() || self.skip_prefixes.iter().any(|p| output.starts_with(p))
    }

    /// Log non-empty lines.
    fn log(&self, name: &str, output: &str) {
        if self.should_skip(output) {
            return;
        }
        for line in output.lines().filter(|s| !s.trim().is_empty()) {
            log!(name; "{line}");
        }
    }
}

/// Log command output, filtering known noise.
fn log_output(name: &str, output: &Output) -> Result<()> {
    let stdout = std::str::from_utf8(&output.stdout)
        .context("Invalid UTF-8 in stdout")?
        .trim();
    let stderr = std::str::from_utf8(&output.stderr)
        .context("Invalid UTF-8 in stderr")?
        .trim();

    if !output.status.success() {
        let cleaned = stderr.trim_start_matches(OutputFilter::STDERR.strip_warning);
        if !cleaned.is_empty() {
            eprintln!("{cleaned}");
        }
        anyhow::bail!("Command `{name}` failed with {}", output.status);
    }

    OutputFilter::STDOUT.log(name, stdout);
    OutputFilter::STDERR.log(name, stderr);

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
