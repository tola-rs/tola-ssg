//! The roff manual page, generated from the CLI's own grammar.
//!
//! Only the title and section are Tola's: every command, flag, and description comes from the same
//! Clap definition that `--help` renders, so the two cannot drift.
use std::path::Path;

use anyhow::{Context, Result};
use clap::CommandFactory;
use clap_mangen::Man;

use crate::cli::Cli;
use crate::cli::output::CommandOutput;
use crate::terminal::display_path_as_given;

/// Write a roff manual page to the requested file or stdout.
pub(in crate::cli) fn run(output_path: Option<&Path>, output: &CommandOutput) -> Result<()> {
    let command = Cli::command();
    // The roff title field takes one line; `--version` names two releases on two lines,
    // so the title carries the release alone.
    let release = command
        .get_version()
        .and_then(|version| version.lines().next())
        .unwrap_or_default()
        .to_owned();
    let man = Man::new(command)
        .title("TOLA")
        .section("1")
        .source(format!("tola {release}"));
    let mut manual = Vec::new();
    man.render(&mut manual)?;
    match output_path {
        Some(path) => {
            crate::cli::log::destination::check_file(output.log().map(|log| log.path()), path)?;
            std::fs::write(path, manual).with_context(|| {
                format!(
                    "Tola could not write the manual page to `{}`",
                    display_path_as_given(path)
                )
            })?;
            output.status(format!(
                "Wrote manual page to {}",
                display_path_as_given(path)
            ))?;
        }
        None => output.write_stdout(manual)?,
    }
    Ok(())
}
