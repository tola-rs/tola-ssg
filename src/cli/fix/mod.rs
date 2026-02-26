//! Fix command - check and repair common issues.

mod check;
mod prompt;

use anyhow::Result;

use crate::config::SiteConfig;
use crate::embed::typst::{TOLA_TEMPLATE, TOLA_UTIL, TolaTypstVars};
use crate::log;

use check::{CheckResult, check_and_fix};

/// Version prefix in tola.typ files: `// Tola SSG ... (vX.X.X)`
pub(super) const VERSION_PATTERN: &str = "(v";

/// GitHub URLs for reference
const GITHUB_TEMPLATE: &str =
    "https://github.com/tola-rs/tola-ssg/blob/main/src/embed/typst/templates/tola.typ";
const GITHUB_UTIL: &str =
    "https://github.com/tola-rs/tola-ssg/blob/main/src/embed/typst/utils/tola.typ";

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
