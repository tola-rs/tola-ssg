#show: config(
  summary: [配置 `tola dev` 与 `tola preview` 的监听地址与端口。],
  related: ("[dev]", "[site]"),
)

可通过命令行 `--interface` 或 `--port` 覆盖单次运行设置（例如 `tola preview --port 0`）。

#example(title: [在本地 8080 端口监听])[
  ```toml
  [server]
  interface = "127.0.0.1"
  port = 8080
  ```
]

#key("interface")[
  监听绑定的 IP 地址。默认 `127.0.0.1` 仅限本机访问；填 `0.0.0.0` 或 `::` 允许来自其他主机的连接。
]

#key("port")[
  监听绑定的 TCP 端口。填 `0` 自动选择空闲端口。
]
