//! `[dev]` section configuration.

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// Development watch settings.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(crate = tola_config, section = "dev")]
pub(crate) struct DevConfig {
    /// Whether `tola dev` watches saved files and reloads the browser.
    pub(crate) watch: bool,
}

impl DevConfig {
    /// What `tola help "[dev]"` adds under its table.
    pub const HELP: &'static str = "\
Three commands take a site from sources to a page you can read. `tola build` builds once, writes
the publish directory, and exits. `tola dev` builds, serves the site locally, and keeps watching
every source it read: an edit rebuilds and refreshes the browser. `tola preview` builds once with
production settings and serves that result locally without watching or writing `publish-dir`, so
you can see the pages about to go live before you commit them.

`watch` belongs to `tola dev`, the only one of the three that watches:

```toml
[dev]
watch = true
```

Run the same site three ways:

```sh
tola build      # write the publish directory and exit
tola dev        # serve locally and rebuild on every edit
tola preview    # serve the production build without watching
```

The listener both serving commands use is `[server]`; `watch = false` runs `tola dev` as a single
build that keeps serving.";
}

impl Default for DevConfig {
    fn default() -> Self {
        Self { watch: true }
    }
}
