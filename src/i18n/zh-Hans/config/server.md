# `[server]`

设置 `tola dev` 和 `tola preview` 的监听地址及端口。

## 配置项

### `interface`

服务器监听的 IP 地址。默认 `127.0.0.1`，只有本机能访问。用 `0.0.0.0` 或 `::` 可接受来自其他机器的连接。

### `port`

监听端口，省略时用 `5277`。写 `0` 自动选一个空闲端口。

## 用法

例如，把端口改为 8080：

```toml
[server]
interface = "127.0.0.1"
port = 8080
```

运行 `tola dev` 或 `tola preview`，打开终端给出的地址。如果 `site.base-path` 用的是默认值，地址就是 `http://127.0.0.1:8080/`。

用 `tola preview --port 0` 为本次预览自动选一个空闲端口。要允许其他机器连接，可用 `--interface 0.0.0.0`。
