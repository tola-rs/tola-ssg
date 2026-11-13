//! Limits for terminal errors and warnings.
//!
//! # Example
//!
//! ```toml
//! [diagnostics]
//! max_errors = 3
//! max_warnings = 3
//! ```

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// Limit how many errors and warnings appear in the terminal.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "diagnostics")]
pub(crate) struct DiagnosticsConfig {
    /// Maximum number of errors to print at a time. If omitted, show up to 3. Use `0` to hide errors.
    pub(crate) max_errors: Option<usize>,

    /// Maximum number of warnings to print at a time. If omitted, show up to 3. Use `0` to hide warnings.
    pub(crate) max_warnings: Option<usize>,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            max_errors: Some(3),
            max_warnings: Some(3),
        }
    }
}

impl DiagnosticsConfig {
    /// What `tola help "[diagnostics]"` adds under its table.
    pub const HELP: &'static str = r#"
To see up to ten errors per build and hide warnings in the terminal, add this to `tola.toml`.

```toml
[diagnostics]
max_errors = 10
max_warnings = 0
```

Run `tola check` from the site root. Errors appear first. If there are more errors or warnings
than the limit, the terminal says how many it left out. Each `tola dev` rebuild starts
counting again.

These limits only affect what you see in the terminal. An error still fails the build.

### Read the full report

From the site root, save the check report and render it with the built-in script:

```sh
tola check --json --pretty > check.json
tola nu --stdin @tola-scripts/diagnostic render < check.json
```

The report includes diagnostics hidden by the terminal limits. Check the exit status of
`tola check` to see whether the check passed. The render command only displays the report.
See [`@tola-scripts/diagnostic`](tola-help://scripts/diagnostic) for the JSON fields."#;
}
