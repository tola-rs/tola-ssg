//! `[serve]` section configuration.
//!
//! Contains development server settings.
//!
//! # Example
//!
//! ```toml
//! [serve]
//! interface = "127.0.0.1"     # Network interface (127.0.0.1 = localhost only)
//! port = 5277                 # HTTP port number
//! watch = true                # Auto-rebuild on file changes
//! respect_prefix = false      # Ignore path_prefix for local development
//! ```
//!
//! Use `interface = "0.0.0.0"` to make the server accessible from LAN.
//!
//! Set `respect_prefix = true` to test deployment paths (e.g., GitHub Pages subdirectory).

use std::net::{IpAddr, Ipv4Addr};

use macros::Config;
use serde::{Deserialize, Serialize};

/// Development server settings
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "serve")]
pub struct ServeConfig {
    /// Network interface to bind
    /// - `127.0.0.1` (default): localhost only
    /// - `0.0.0.0`: all interfaces (LAN accessible)
    pub interface: IpAddr,

    #[config(inline_doc = "HTTP port number")]
    pub port: u16,

    #[config(inline_doc = "Enable file watcher for live reload")]
    pub watch: bool,

    /// Respect path_prefix from site.url during local development
    /// - `false` (default): Ignore prefix, access pages at `/`
    /// - `true`: Keep prefix, access at `/my-project/`
    pub respect_prefix: bool,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            interface: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            port: 5277,
            watch: true,
            respect_prefix: false,
        }
    }
}
