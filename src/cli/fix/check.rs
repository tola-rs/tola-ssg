use anyhow::Result;
use std::fs;
use std::path::Path;

use super::{VERSION_PATTERN, prompt::prompt_create};
use crate::logger;

/// File check result
pub(super) enum CheckResult {
    /// File is up to date
    Ok,
    /// File is missing, user chose to create
    Created,
    /// File is missing, user declined
    Skipped,
    /// File exists but version is outdated or missing
    Outdated,
}

/// Check file and fix if needed
pub(super) fn check_and_fix(
    path: &Path,
    name: &str,
    current_version: &str,
    github_url: &str,
    generate: impl FnOnce() -> String,
) -> Result<CheckResult> {
    // Case: File missing -> prompt to create
    if !path.exists() {
        logger::log("fix", format_args!("{} not found", name));
        if prompt_create(name)? {
            fs::write(path, generate())?;
            logger::log("fix", format_args!("created {}", name));
            return Ok(CheckResult::Created);
        }
        return Ok(CheckResult::Skipped);
    }

    // Case: File exists -> check version
    match extract_version(path)? {
        Some(v) if v == current_version => Ok(CheckResult::Ok),
        Some(v) => {
            logger::log(
                "fix",
                format_args!("{}: v{} -> v{} available", name, v, current_version),
            );
            logger::log("fix", format_args!("see `{}`", github_url));
            Ok(CheckResult::Outdated)
        }
        None => {
            logger::log("fix", format_args!("{}: no version marker", name));
            logger::log("fix", format_args!("see `{}`", github_url));
            Ok(CheckResult::Outdated)
        }
    }
}

/// Extract version from file's first line: `// Tola SSG ... (vX.X.X)`
fn extract_version(path: &Path) -> Result<Option<String>> {
    let content = fs::read_to_string(path)?;
    let first_line = content.trim().lines().next().unwrap_or("");

    // Find "(vX.X.X)" pattern
    if let Some(start) = first_line.find(VERSION_PATTERN) {
        let after = &first_line[start + VERSION_PATTERN.len()..];
        if let Some(end) = after.find(')') {
            return Ok(Some(after[..end].to_string()));
        }
    }
    Ok(None)
}
