//! Fix command - check and repair common issues.

use anyhow::Result;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::config::SiteConfig;
use crate::embed::typst::{TOLA_TEMPLATE, TOLA_UTIL, TolaTypstVars};
use crate::log;

/// Version prefix in tola.typ files: `// Tola SSG ... (vX.X.X)`
const VERSION_PATTERN: &str = "(v";

/// GitHub URLs for reference
const GITHUB_TEMPLATE: &str =
    "https://github.com/tola-rs/tola-ssg/blob/main/src/embed/typst/templates/tola.typ";
const GITHUB_UTIL: &str =
    "https://github.com/tola-rs/tola-ssg/blob/main/src/embed/typst/utils/tola.typ";

/// File check result
enum CheckResult {
    /// File is up to date
    Ok,
    /// File is missing, user chose to create
    Created,
    /// File is missing, user declined
    Skipped,
    /// File exists but version is outdated or missing
    Outdated,
}

/// Run the fix command
pub fn run_fix(config: &SiteConfig) -> Result<()> {
    let root = config.get_root();
    let deps = &config.build.deps;
    let current_version = env!("CARGO_PKG_VERSION");

    let mut has_issues = false;

    // Check templates/tola.typ if "templates" is in deps
    let templates_dir = root.join("templates");
    if deps.iter().any(|d| d == &templates_dir) && templates_dir.is_dir() {
        let result = check_and_fix(
            &templates_dir.join("tola.typ"),
            "templates/tola.typ",
            current_version,
            GITHUB_TEMPLATE,
            || TOLA_TEMPLATE.render(&TolaTypstVars::default()),
        )?;
        has_issues |= !matches!(result, CheckResult::Ok);
    }

    // Check utils/tola.typ if "utils" is in deps
    let utils_dir = root.join("utils");
    if deps.iter().any(|d| d == &utils_dir) && utils_dir.is_dir() {
        let result = check_and_fix(
            &utils_dir.join("tola.typ"),
            "utils/tola.typ",
            current_version,
            GITHUB_UTIL,
            || TOLA_UTIL.render(&TolaTypstVars::default()),
        )?;
        has_issues |= !matches!(result, CheckResult::Ok);
    }

    if !has_issues {
        log!("fix"; "all files up to date");
    }

    Ok(())
}

// =============================================================================
// Core logic
// =============================================================================

/// Check file and fix if needed
fn check_and_fix(
    path: &Path,
    name: &str,
    current_version: &str,
    github_url: &str,
    generate: impl FnOnce() -> String,
) -> Result<CheckResult> {
    // Case 1: File missing → prompt to create
    if !path.exists() {
        log!("fix"; "{} not found", name);
        if prompt_create(name)? {
            fs::write(path, generate())?;
            log!("fix"; "created {}", name);
            return Ok(CheckResult::Created);
        }
        return Ok(CheckResult::Skipped);
    }

    // Case 2: File exists → check version
    match extract_version(path)? {
        Some(v) if v == current_version => Ok(CheckResult::Ok),
        Some(v) => {
            log!("fix"; "{}: v{} → v{} available", name, v, current_version);
            log!("fix"; "see `{}`", github_url);
            Ok(CheckResult::Outdated)
        }
        None => {
            log!("fix"; "{}: no version marker", name);
            log!("fix"; "see `{}`", github_url);
            Ok(CheckResult::Outdated)
        }
    }
}

// =============================================================================
// Helpers (pure functions)
// =============================================================================

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

// =============================================================================
// IO (side effects)
// =============================================================================

/// Prompt user to create file
fn prompt_create(name: &str) -> Result<bool> {
    eprint!("Create {}? [y/N] ", name);
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let input = input.trim().to_lowercase();
    Ok(input == "y" || input == "yes")
}
