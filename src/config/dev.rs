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

impl Default for DevConfig {
    fn default() -> Self {
        Self { watch: true }
    }
}
