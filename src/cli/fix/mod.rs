//! Fix command - check and repair common issues.

mod check;
mod prompt;

use anyhow::Result;

use crate::config::SiteConfig;
use crate::embed::typst::{TOLA_LIB, TolaTypstVars};

use crate::logger;
use check::{CheckResult, check_and_fix};

/// Version prefix in generated Tola Typst files: `// Tola SSG ... (vX.X.X)`
pub(super) const VERSION_PATTERN: &str = "(v";

/// GitHub URLs for reference
const GITHUB_LIB: &str =
    "https://github.com/tola-rs/tola-ssg/blob/main/src/embed/typst/tola/lib.typ";

/// Run the fix command
pub fn run_fix(config: &SiteConfig) -> Result<()> {
    let root = config.get_root();
    let deps = &config.build.deps;
    let current_version = env!("CARGO_PKG_VERSION");

    let mut has_issues = false;

    let tola_dir = root.join("tola");
    if deps.iter().any(|d| d == &tola_dir) && tola_dir.is_dir() {
        let result = check_and_fix(
            &tola_dir.join("lib.typ"),
            "tola/lib.typ",
            current_version,
            GITHUB_LIB,
            || TOLA_LIB.render(&TolaTypstVars::default()),
        )?;
        has_issues |= !matches!(result, CheckResult::Ok);
    }

    if !has_issues {
        logger::log("fix", format_args!("all files up to date"));
    }

    Ok(())
}
