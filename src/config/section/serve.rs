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

/// Development server settings.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "serve")]
pub struct ServeConfig {
    /// Network interface to bind.
    /// - `127.0.0.1` (default): localhost only
    /// - `0.0.0.0`: all interfaces (LAN accessible)
    pub interface: IpAddr,

    /// HTTP port number.
    #[config(inline_doc = "HTTP port number.")]
    pub port: u16,

    /// Enable file watcher for live reload.
    #[config(inline_doc = "Enable file watcher for live reload.")]
    pub watch: bool,

    /// Respect path_prefix from site.url during local development.
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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use crate::config::test_parse_config;

    #[test]
    fn test_serve_config() {
        let config =
            test_parse_config("[serve]\ninterface = \"0.0.0.0\"\nport = 8080\nwatch = false");

        assert_eq!(
            config.serve.interface,
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
        );
        assert_eq!(config.serve.port, 8080);
        assert!(!config.serve.watch);
    }

    #[test]
    fn test_serve_config_defaults() {
        let config = test_parse_config("");

        assert_eq!(
            config.serve.interface,
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(config.serve.port, 5277);
        assert!(config.serve.watch);
    }

    #[test]
    fn test_serve_config_interface_variants() {
        // Test IPv4 any
        let config = test_parse_config("[serve]\ninterface = \"0.0.0.0\"");
        assert_eq!(
            config.serve.interface,
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
        );

        // Test IPv6 localhost
        let config = test_parse_config("[serve]\ninterface = \"::1\"");
        assert_eq!(
            config.serve.interface,
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))
        );
    }

    #[test]
    fn test_serve_config_port_range() {
        // Test minimum port
        let config = test_parse_config("[serve]\nport = 1");
        assert_eq!(config.serve.port, 1);

        // Test maximum port
        let config = test_parse_config("[serve]\nport = 65535");
        assert_eq!(config.serve.port, 65535);
    }

    #[test]
    fn test_serve_config_watch_disabled() {
        let config = test_parse_config("[serve]\nwatch = false");
        assert!(!config.serve.watch);
    }

    #[test]
    fn test_serve_config_partial_override() {
        let config = test_parse_config("[serve]\nport = 3000");

        // port is overridden
        assert_eq!(config.serve.port, 3000);
        // interface uses default
        assert_eq!(
            config.serve.interface,
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        );
        // watch uses default
        assert!(config.serve.watch);
    }
}
