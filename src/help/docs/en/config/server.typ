#show: config(
  summary: [Configure the listen address and port used by `tola dev` and `tola preview`.],
  related: ("[dev]", "[site]"),
)

Override either setting for a single run with `--interface` or `--port` (for example `tola preview --port 0`).

#example(title: [Listen on port 8080 locally])[
  ```toml
  [server]
  interface = "127.0.0.1"
  port = 8080
  ```
]

#key("interface")[
  IP address to bind. Defaults to `127.0.0.1` (loopback only); use `0.0.0.0` or `::` to accept connections from other hosts.
]

#key("port")[
  TCP port to bind. Use `0` to pick an available ephemeral port automatically.
]
