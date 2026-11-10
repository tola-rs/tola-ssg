//! `tola edit`: read site files with their digests, and apply a JSON edit batch under the
//! preconditions it carries.
//!
//! A batch states the digest, or the absence, of every file it touches, so `apply` refuses a file
//! that changed after the batch was prepared rather than overwriting it. `check` validates the same
//! batch and writes nothing.
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::json;

use crate::cancellation::Cancellation;
use crate::cli::ConfigFileArgs;
use crate::cli::log::destination;
use crate::cli::output::CommandOutput;
use crate::edits::EditBatch;

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum EditCommand {
    /// Read project-relative UTF-8 files and their exact content digests as JSON.
    Read {
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
    },
    /// Validate a JSON edit batch without writing its changes; use - for stdin.
    Check { edits: PathBuf },
    /// Apply a JSON edit batch if its input files are unchanged; use - for stdin.
    Apply { edits: PathBuf },
}

pub(in crate::cli) fn run(
    config: ConfigFileArgs,
    command: EditCommand,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let root = tola_build::config::loading::config_site_root(config.path.as_deref())?;
    cancellation.token().ensure_active()?;
    let log = output.log().map(|log| log.path());
    let (path, apply) = match command {
        EditCommand::Read { paths } => {
            for path in &paths {
                destination::check_file(log, &root.join(path))?;
            }
            let files = crate::edits::read(&root, &paths)?;
            destination::start(output, &cancellation.token())?;
            return output.write_stdout_line(serde_json::to_string(&json!({
                "schemaVersion":1, "root":root, "files":files
            }))?);
        }
        EditCommand::Check { edits } => (edits, false),
        EditCommand::Apply { edits } => (edits, true),
    };
    let bytes = if path.as_os_str() == "-" {
        crate::cli::input::read_stdin(cancellation)?
    } else {
        destination::check_file(log, &path)?;
        std::fs::read(&path).with_context(|| {
            let shown = tola_diagnostic::display_path(&path, &root);
            format!("could not read edit batch `{shown}`")
        })?
    };
    let edits: EditBatch =
        serde_json::from_slice(&bytes).context("the edit batch is not valid JSON")?;
    for path in edits.paths() {
        destination::check_file(log, &root.join(path))?;
    }
    let prepared = edits.prepare(&root)?;
    destination::start(output, &cancellation.token())?;
    if apply {
        prepared.apply(&cancellation.token())?;
    }
    let changes = edits
        .changes
        .iter()
        .map(|change| {
            json!({
                "path":change.path, "before":change.before,
                "after":crate::edits::digest(change.text.as_bytes())
            })
        })
        .collect::<Vec<_>>();
    output.write_stdout_line(serde_json::to_string(&json!({
        "schemaVersion":1, "root":root,
        "status":if apply {"applied"} else {"checked"}, "changes":changes
    }))?)
}
