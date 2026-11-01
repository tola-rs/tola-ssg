//! Run command-line tools with inherited streams and the CLI's cancellation policy.

use std::process::ExitStatus;

use anyhow::{Context, Result, bail};
use tola_subprocess::{Command, Stop};

use crate::cancellation::Cancellation;

#[derive(Debug)]
pub(super) struct CommandExit(pub ExitStatus);

impl std::fmt::Display for CommandExit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "command exited with {}", self.0)
    }
}

impl std::error::Error for CommandExit {}

/// The executable this process runs, which child commands re-invoke.
pub(super) fn current_executable() -> Result<std::path::PathBuf> {
    std::env::current_exe().context("Tola could not find its own executable")
}

pub(super) fn run(command: Command, cancellation: &Cancellation) -> Result<()> {
    let token = cancellation.token();
    let stopped = command
        .run_inherited(&|| token.is_cancelled())
        .with_context(|| {
            format!(
                "Tola could not run `{}`",
                std::path::Path::new(command.program()).display()
            )
        })?;
    let status = match stopped {
        Stop::Exited(status) => status,
        Stop::Cancelled(status) => {
            if status.is_some() {
                crate::terminal::restore_external_screen();
            }
            token.ensure_active()?;
            bail!("command was cancelled");
        }
        _ => bail!("command returned an unsupported completion state"),
    };
    if !status.success() {
        return Err(CommandExit(status).into());
    }
    Ok(())
}
