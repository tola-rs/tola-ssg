//! The few argv positions clap cannot own.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::ColorChoice;

use super::args::{InputArgs, ProcessArgs};

/// The colour choice this invocation names, when it names one.
///
/// Clap renders its own errors before parsing succeeds, so the choice has to be read from the
/// raw arguments first.
pub(super) fn color_hint(arguments: &[OsString]) -> ColorChoice {
    let mut color = ColorChoice::Auto;
    let mut index = 1;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        if argument == "--" {
            break;
        }
        let value = if argument == "--color" {
            index += 1;
            arguments.get(index).map(|value| value.to_string_lossy())
        } else {
            argument.strip_prefix("--color=").map(Into::into)
        };
        if let Some(value) = value {
            color = match value.as_ref() {
                "always" => ColorChoice::Always,
                "never" => ColorChoice::Never,
                "auto" => ColorChoice::Auto,
                _ => color,
            };
        }
        index += 1;
    }
    color
}

/// Consume the process and input options a module invocation carries before its command name.
pub(super) fn adopt_prefix(
    process: &mut ProcessArgs,
    input: &mut InputArgs,
    arguments: &mut Vec<OsString>,
) -> Result<(), String> {
    while let Some(argument) = arguments
        .first()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
    {
        let mut consumed = 1;
        match argument.as_str() {
            "--" => break,
            "--color" => {
                let value = value_at(arguments, 1, "--color")?
                    .to_str()
                    .ok_or_else(|| "--color requires a UTF-8 value".to_owned())?;
                process.color = ProcessArgs::parse_color(value)?;
                consumed = 2;
            }
            value if value.starts_with("--color=") => {
                process.color = ProcessArgs::parse_color(&value["--color=".len()..])?;
            }
            "--log-file" => {
                process.log_file = Some(PathBuf::from(value_at(arguments, 1, "--log-file")?));
                consumed = 2;
            }
            value if value.starts_with("--log-file=") => {
                process.log_file = Some(PathBuf::from(&value["--log-file=".len()..]));
            }
            "--offline" => input.offline = true,
            "--pure" => input.pure = true,
            "--locked" => input.locked = true,
            "--no-log-file" => process.no_log_file = true,
            "-q" | "--quiet" => process.quiet = true,
            "-v" | "--verbose" => process.verbose = process.verbose.saturating_add(1),
            value if value.starts_with("-v") && value[1..].chars().all(|c| c == 'v') => {
                process.verbose = process.verbose.saturating_add((value.len() - 1) as u8);
            }
            _ => break,
        }
        if process.log_file.is_some() && process.no_log_file {
            return Err("--log-file conflicts with --no-log-file".to_owned());
        }
        arguments.drain(0..consumed);
    }
    Ok(())
}

/// Consume the leading `-C`/`--config` option a module invocation carries.
pub(super) fn take_config(arguments: &mut Vec<OsString>) -> Result<Option<PathBuf>, String> {
    let mut config = None;
    loop {
        let path = if arguments
            .first()
            .is_some_and(|value| value == "--config" || value == "-C")
        {
            arguments.remove(0);
            PathBuf::from(value_at(arguments, 0, "--config")?)
        } else if let Some(path) = arguments
            .first()
            .and_then(|value| value.to_str())
            .and_then(|value| value.strip_prefix("--config="))
        {
            let path = PathBuf::from(path);
            arguments.remove(0);
            path
        } else {
            break;
        };
        if path.as_os_str().is_empty() {
            return Err("--config requires a path".to_owned());
        }
        if config.is_some() {
            return Err("--config may be supplied only once".to_owned());
        }
        config = Some(path);
    }
    Ok(config)
}

fn value_at<'a>(
    arguments: &'a [OsString],
    index: usize,
    option: &str,
) -> Result<&'a OsString, String> {
    arguments
        .get(index)
        .ok_or_else(|| format!("{option} requires a value"))
}
