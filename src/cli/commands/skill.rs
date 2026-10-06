//! Export the bundled Tola authoring skill without loading a site or configuring a client.

use std::path::Path;

use anyhow::Result;

use crate::cancellation::Cancellation;
use crate::cli::log::{LogFile, destination};
use crate::cli::output::CommandOutput;
use crate::terminal::display_path_as_given;
use crate::writes::FileWrites;

const SKILL: &str = include_str!("../../../.agents/skills/tola/SKILL.md");

pub(in crate::cli) fn run(
    output_root: Option<&Path>,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let cancellation = cancellation.token();
    cancellation.ensure_active()?;
    let Some(root) = output_root else {
        destination::start(output, &cancellation)?;
        return output.write_stdout(SKILL);
    };

    let display = root.join("tola");
    let root = std::env::current_dir()?.join(root);
    let mut writes = FileWrites::new(&root)?;
    writes.create_file("tola/SKILL.md", SKILL)?;
    destination::check_writes(output.log().map(LogFile::path), &writes, false)?;
    writes.check()?;
    destination::start(output, &cancellation)?;
    writes.apply(&cancellation)?;
    output.status(format!(
        "Wrote Tola skill to {}",
        display_path_as_given(&display)
    ))?;
    Ok(())
}
