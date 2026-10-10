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

    fn rendered_manual() -> String {
        let (sink, manual) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        run(None, &output).unwrap();
        String::from_utf8(manual.bytes()).unwrap()
    }

    #[test]
    fn manpage_lists_every_verb() {
        let manual = rendered_manual();
        for command in Cli::command().get_subcommands() {
            let name = command.get_name();
            assert!(manual.contains(name), "command={name}");
        }
    }

    #[test]
    fn manpage_title_request_stays_one_line() {
        let manual = rendered_manual();
        let mut lines = manual.lines();
        let title = lines
            .find(|line| line.starts_with(".TH "))
            .expect("the manual opens with a title request");
        assert!(title.contains(env!("CARGO_PKG_VERSION")), "{title}");
        assert_eq!(lines.next(), Some(".SH NAME"));
    }
}
