//! `[dev]` section configuration.

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// Control when `tola dev` rebuilds the site and reloads the browser.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(crate = tola_config, section = "dev")]
pub(crate) struct DevConfig {
    /// Watch files and reload the browser after a rebuild. Omit this setting to leave watching on.
    pub(crate) watch: bool,
}

impl DevConfig {
    /// What `tola help "[dev]"` adds under its table.
    pub const HELP: &'static str = r#"
File watching and browser reload are on by default. To turn them off for the site, add this to
`tola.toml`.

```toml
[dev]
watch = false
```

Run `tola dev` from the site root and open the printed URL. With watching off, the server keeps
serving the first build when you save a file. For one run, use `tola dev --watch=false`.

`tola dev` always builds in `dev` mode, and each hook follows its `dev` setting. To serve one
production build, use `tola preview`. Both commands use `[server]`.

### Check a hook while editing

When changes to a data file should trigger a rebuild, add its path to the hook's `rerun-on`.
Tola already watches the script named by `nu`. Start `tola dev --no-reuse` to run
`before-build` and `generate-outputs` hooks configured with `dev = "reuse"` on every build.
See [`[hooks]`](tola-help://config/hooks) for the stages and their options."#;
}

impl Default for DevConfig {
    fn default() -> Self {
        Self { watch: true }
    }
}
