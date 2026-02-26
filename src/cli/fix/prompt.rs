use anyhow::Result;
use std::io::{self, Write};

/// Prompt user to create file
pub(super) fn prompt_create(name: &str) -> Result<bool> {
    eprint!("Create {}? [y/N] ", name);
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let input = input.trim().to_lowercase();
    Ok(input == "y" || input == "yes")
}
