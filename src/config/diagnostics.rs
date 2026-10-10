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

/// Caps how many diagnostics the terminal shows.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "diagnostics")]
pub(crate) struct DiagnosticsConfig {
    /// Maximum errors shown per terminal batch; the default is 3.
    pub(crate) max_errors: Option<usize>,

    /// Maximum warnings shown per terminal batch; the default is 3.
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
    pub const HELP: &'static str = "\
`max_errors` and `max_warnings` limit how many diagnostics one batch of terminal output shows.
Errors print before warnings, each severity counted against its own limit, and every development
rebuild starts a new batch. Terminal positions follow the Typst CLI: lines count from 1 and
columns from 0.

```toml
[diagnostics]
max_errors = 3
max_warnings = 3
```

Set a limit to `0` to hide that severity from the terminal — that is terminal output only: the
`--log-file` record and the diagnostics your editor gets over LSP stay complete. Paths in the
terminal render relative to where you run the command; the editor lists diagnostics at the open file
and its lines. A batch that exceeds a limit ends by naming how many diagnostics it hid:

```
3 errors not shown; increase `diagnostics.max_errors` to show more
```

`[diagnostics]` does not change the outcome of the build itself.";
}
