//! Color for parse errors, which can occur before clap returns any matches.

use std::ffi::OsString;

use clap::ColorChoice;

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
