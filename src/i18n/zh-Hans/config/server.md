# `[server]`

`tola dev` 与 `tola preview` 监听的 HTTP 服务器设置。

## 配置项

### `interface`

要绑定的 IP 地址。默认回环地址最安全；`0.0.0.0`、`::` 这类未指定地址会把服务器暴露到网络上。

### `port`

服务器监听的端口。写 `0` 让操作系统挑一个空闲端口。

## 用法

`tola dev` 与 `tola preview` 绑定这张表的监听器：`tola build` 只写文件，永远不开端口。

`interface` 是要绑定的地址，默认回环地址，只有本机能访问；写成 `0.0.0.0`、`::` 这类未指定地址，就会接受网络上的连接。`port` 是要监听的端口，写 `0` 让操作系统挑一个空闲端口：

```toml
[server]
interface = "127.0.0.1"    # this machine only; "0.0.0.0" accepts the network
port = 5277               # 0 asks the operating system for a free port
```

两个命令都接受 `--interface` 与 `--port` 进行覆盖：

```sh
tola dev --interface 0.0.0.0 --port 8080
tola preview --port 0
```

两个命令共用这个监听器；文件变动时重建并刷新浏览器只有 `tola dev` 会做，它的开关是 `[dev]` 的 `watch`。
