//! `[server]` section configuration.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// Set the address and port for `tola dev` and `tola preview`.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(crate = tola_config, section = "server")]
pub(crate) struct ServerConfig {
    /// IP address to listen on. If omitted, use `127.0.0.1` so only this machine can connect.
    /// Use `0.0.0.0` or `::` to accept connections from other machines.
    pub(crate) interface: IpAddr,

    /// Port to listen on. If omitted, use `5277`. Set `0` to pick a free port.
    pub(crate) port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            interface: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 5277,
        }
    }
}

impl ServerConfig {
    /// What `tola help "[server]"` adds under its table.
    pub const HELP: &'static str = r#"
To use port 8080 on your machine, add this to `tola.toml`.

```toml
[server]
interface = "127.0.0.1"
port = 8080
```

Run `tola dev` or `tola preview` from the site root and open the printed URL. With the default
`site.base-path`, it is `http://127.0.0.1:8080/`.

`tola preview --port 0` picks a free port for one run. Set `--interface 0.0.0.0` to accept
connections from other machines."#;
}

/// Overrides for the `[server]` section.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServerOverrides {
    pub(crate) interface: Option<IpAddr>,
    pub(crate) port: Option<u16>,
}
