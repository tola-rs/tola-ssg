//! `[server]` section configuration.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// HTTP listener settings shared by the development and preview servers.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(crate = tola_config, section = "server")]
pub(crate) struct ServerConfig {
    /// IP address to bind. Loopback is the safe default; unspecified addresses
    /// such as `0.0.0.0` and `::` expose the server to the network.
    pub(crate) interface: IpAddr,

    /// Port the server listens on. `0` asks the operating system for a free port.
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
    /// What `tola help config server` adds under its table.
    pub const HELP: &'static str = "\
The listener `tola dev` and `tola preview` bind: `tola build` writes files and never opens a port.

`interface` is the address to bind. It defaults to loopback, reachable only from your own
machine; an unspecified address such as `0.0.0.0` or `::` accepts connections from the network
instead. `port` is the port to listen on, and `0` asks the operating system for a free one:

```toml
[server]
interface = \"127.0.0.1\"    # this machine only; \"0.0.0.0\" accepts the network
port = 5277               # 0 asks the operating system for a free port
```

Both commands take `--interface` and `--port` to override this table:

```sh
tola dev --interface 0.0.0.0 --port 8080
tola preview --port 0
```

The two commands that serve share this listener; only `tola dev` rebuilds and reloads on file
changes, and its switch is `[dev]`'s `watch`.";
}

/// Overrides for the `[server]` section.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServerOverrides {
    pub(crate) interface: Option<IpAddr>,
    pub(crate) port: Option<u16>,
}
