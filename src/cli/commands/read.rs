//! Selection and JSON output shared by read-only commands.

use anyhow::Result;
use serde_json::Value;
use tola_build::InputScope;

use crate::cancellation::Cancellation;
use crate::cli::args::JsonOutputArgs;
use crate::cli::output::CommandOutput;

pub(super) fn check_output(args: &JsonOutputArgs, output: &CommandOutput) -> Result<()> {
    if let Some(path) = &args.output {
        crate::cli::log::destination::check_file(output.log().map(|log| log.path()), path)?;
    }
    Ok(())
}

pub(super) fn select(
    value: &Value,
    title: &str,
    args: &JsonOutputArgs,
    scope: InputScope,
    cancellation: &Cancellation,
) -> Result<Option<Value>> {
    cancellation.token().ensure_active()?;
    if !args.interactive
        || args.output.is_some()
        || !crate::cli::interaction::available()
        || value.as_array().is_some_and(Vec::is_empty)
    {
        return Ok(Some(value.clone()));
    }
    let array = value.is_array();
    let input = if array {
        serde_json::json!({"title": title, "rows": value})
    } else {
        serde_json::json!({"title": title, "value": value})
    };
    let selected: Option<Value> =
        crate::cli::interaction::run(include_str!("browse.nu"), &input, scope, cancellation)?;
    Ok(selected.map(|selected| {
        if array {
            Value::Array(vec![selected])
        } else {
            selected
        }
    }))
}

pub(super) fn write_json(
    value: &Value,
    args: &JsonOutputArgs,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let encoded = if args.pretty {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    cancellation.token().ensure_active()?;
    if let Some(path) = &args.output {
        tola_build::filesystem::atomic_write(path, format!("{encoded}\n").as_bytes())?;
    } else {
        output.write_stdout_line(encoded)?;
    }
    Ok(())
}

pub(super) fn write(
    value: &Value,
    title: &str,
    args: &JsonOutputArgs,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    check_output(args, output)?;
    if let Some(selected) = select(value, title, args, scope, cancellation)? {
        write_json(&selected, args, output, cancellation)?;
    }
    Ok(())
}
