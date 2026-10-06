use anyhow::Result;
use std::io;

use crate::logger;

/// Prompt user to create file
pub(super) fn prompt_create(name: &str) -> Result<bool> {
    logger::prompt(format_args!("Create {}? [y/N] ", name))?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let input = input.trim().to_lowercase();
    Ok(input == "y" || input == "yes")
}
