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
    let man = Man::new(command).title("TOLA").section("1");
    let mut manual = Vec::new();
    man.render(&mut manual)?;
    match output_path {
        Some(path) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manpage_lists_every_verb() {
        let (sink, manual) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        run(None, &output).unwrap();
        let manual = String::from_utf8(manual.bytes()).unwrap();
        for command in Cli::command().get_subcommands() {
            let name = command.get_name();
            assert!(manual.contains(name), "command={name}");
        }
    }
}
