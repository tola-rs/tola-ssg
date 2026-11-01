//! A terminal script reads JSON input and writes its result to a separate file.

use std::fs;
use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tola_build::InputScope;
use tola_subprocess::Command;

use crate::cancellation::Cancellation;

pub(super) fn available() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum Response<T> {
    Submit { selected: T },
    Quit,
}

pub(super) fn run<T: DeserializeOwned>(
    script: &str,
    input: &impl Serialize,
    scope: InputScope,
    cancellation: &Cancellation,
) -> Result<Option<T>> {
    ensure!(
        available(),
        "interactive mode requires terminal input and output"
    );
    cancellation.token().ensure_active()?;
    let temporary =
        tempfile::tempdir().context("Tola could not create its terminal session files")?;
    let script_path = temporary.path().join("main.nu");
    let input_path = temporary.path().join("input.json");
    let result_path = temporary.path().join("result.json");
    fs::write(&script_path, script)?;
    fs::write(&input_path, serde_json::to_vec(input)?)?;
    io::stdout().flush()?;
    io::stderr().flush()?;
    let executable = super::external::current_executable()?;
    let mut command = Command::new(executable)
        .arg("nu")
        .arg("--no-site")
        .arg(script_path)
        .arg(input_path)
        .arg(&result_path)
        .env("TOLA_INPUT_SCOPE", scope.as_str());
    for (name, value) in super::theme::child_environment() {
        command = command.env(name, value);
    }
    super::external::run(command, cancellation)?;
    cancellation.token().ensure_active()?;
    let bytes = fs::read(&result_path).context("the terminal script returned no result")?;
    match serde_json::from_slice(&bytes)
        .context("the terminal script returned an unexpected result")?
    {
        Response::Submit { selected } => Ok(Some(selected)),
        Response::Quit => Ok(None),
    }
}
