use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{Shell, generate};
use clap_complete_nushell::Nushell;

use crate::cli::output::CommandOutput;
use crate::cli::{Cli, CompletionShell};

/// Write a completion script for one supported shell to stdout.
pub(in crate::cli) fn run(shell: CompletionShell, output: &CommandOutput) -> Result<()> {
    let mut command = Cli::command();
    let command_name = command.get_name().to_owned();
    let mut script = Vec::new();
    match shell {
        CompletionShell::Bash => generate(Shell::Bash, &mut command, &command_name, &mut script),
        CompletionShell::Elvish => {
            generate(Shell::Elvish, &mut command, &command_name, &mut script)
        }
        CompletionShell::Fish => generate(Shell::Fish, &mut command, &command_name, &mut script),
        CompletionShell::Nushell => generate(Nushell, &mut command, &command_name, &mut script),
        CompletionShell::PowerShell => {
            generate(Shell::PowerShell, &mut command, &command_name, &mut script)
        }
        CompletionShell::Zsh => generate(Shell::Zsh, &mut command, &command_name, &mut script),
    }
    output.write_stdout(script)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_cover_every_verb() {
        for shell in <CompletionShell as clap::ValueEnum>::value_variants() {
            let (sink, generated) = crate::terminal::OutputSink::buffered();
            let output = CommandOutput::new(
                crate::terminal::Terminal::with_sink(sink, false, false),
                None,
            );
            run(*shell, &output).unwrap();
            let script = String::from_utf8(generated.bytes()).unwrap();
            for command in Cli::command().get_subcommands() {
                let name = command.get_name();
                assert!(script.contains(name), "shell={shell:?}, command={name}");
            }
        }
    }
}
