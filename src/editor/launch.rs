//! Explicit editor and desktop opens for a persistent exported site.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use anyhow::Result;
use tola_build::diagnostic::{Diagnostic, DiagnosticError, Severity};

use crate::command_line::CommandLine;

pub(crate) fn edit(
    path: &Path,
    command: Option<&str>,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<()> {
    let command = select_editor(command, |name| std::env::var_os(name))?;
    let path = std::path::absolute(path).map_err(|error| {
        tracing::debug!(?error, "could not resolve the editor target");
        failure(
            "could not locate the exported site",
            "Open the export directory in your editor",
        )
    })?;
    let status = crate::sys::run_command(
        std::process::Command::new(&command.program)
            .args(&command.arguments)
            .arg(&path),
        cancellation,
    )
    .map_err(|error| {
        if cancellation.is_cancelled() {
            return anyhow::Error::new(tola_build::cancellation::BuildCancelled);
        }
        tracing::debug!(?error, "could not start the selected editor");
        failure(
            format!(
                "could not start the editor `{}`",
                command.program.to_string_lossy()
            ),
            "Check the editor command; the exported site remains available",
        )
    })?;
    if !status.success() {
        return Err(failure(
            "the editor exited without completing",
            "Open the exported site directly, or choose another editor command",
        ));
    }
    Ok(())
}

fn select_editor(
    explicit: Option<&str>,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Result<CommandLine> {
    let selected = explicit
        .map(|value| ("--editor", OsString::from(value)))
        .or_else(|| {
            ["TOLA_EDITOR", "VISUAL", "EDITOR"]
                .into_iter()
                .find_map(|name| lookup(name).map(|value| (name, value)))
        });
    let Some((name, value)) = selected else {
        return Err(failure(
            "no editor command is configured",
            "Set TOLA_EDITOR, VISUAL, or EDITOR, or pass --editor with --edit",
        ));
    };
    CommandLine::parse(&value)
        .map_err(|error| {
            failure(
                format!("`{name}` is not an editor command: {error}"),
                "Quote arguments that contain spaces; shell expansion is not performed",
            )
        })?
        .ok_or_else(|| {
            failure(
                format!("`{name}` is empty"),
                "Set an editor command, or open the exported site yourself",
            )
        })
}

pub(crate) fn open_url(
    address: &str,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<()> {
    let parsed = url::Url::parse(address)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"));
    if parsed.is_none() {
        return Err(failure(
            "the preview address is not an HTTP URL",
            "Restart the demo preview",
        ));
    }
    open_target(
        OsStr::new(address),
        "could not open the preview in a browser",
        cancellation,
    )
}

fn open_target(
    target: &OsStr,
    message: &'static str,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<()> {
    crate::sys::open_default(target, cancellation).map_err(|error| {
        if cancellation.is_cancelled() {
            return anyhow::Error::new(tola_build::cancellation::BuildCancelled);
        }
        tracing::debug!(?error, "the default application did not open");
        failure(
            message,
            "Open the displayed address or exported path directly",
        )
    })
}

fn failure(message: impl Into<String>, help: &'static str) -> anyhow::Error {
    let message = message.into();
    DiagnosticError::new(
        message.clone(),
        vec![
            Diagnostic::new(crate::codes::editor::LAUNCH, Severity::Error, message).with_help(help),
        ],
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_selection_respects_precedence() {
        let environment = |name: &str| match name {
            "TOLA_EDITOR" => Some("tola-editor --wait".into()),
            "VISUAL" => Some("visual-editor".into()),
            "EDITOR" => Some("text-editor".into()),
            _ => None,
        };
        assert_eq!(
            select_editor(Some("chosen"), environment).unwrap().program,
            "chosen"
        );
        assert_eq!(
            select_editor(None, environment).unwrap().program,
            "tola-editor"
        );
        let generic = |name: &str| {
            if name == "TOLA_EDITOR" {
                None
            } else {
                environment(name)
            }
        };
        assert_eq!(
            select_editor(None, generic).unwrap().program,
            "visual-editor"
        );
        assert!(select_editor(None, |_| None).is_err());
    }

    #[test]
    fn empty_editor_does_not_fall_through() {
        assert!(
            select_editor(None, |name| {
                Some(if name == "TOLA_EDITOR" { "" } else { "editor" }.into())
            })
            .is_err()
        );
    }
}
