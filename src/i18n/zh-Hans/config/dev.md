# `[dev]`

设置 `tola dev` 是否在文件变化后重建并刷新浏览器。

## 配置项

### `watch`

默认开启。`tola dev` 会在文件变化后重建站点，并刷新浏览器。

## 用法

在 `tola.toml` 里关闭文件监听：

```toml
[dev]
watch = false
```

运行 `tola dev`，打开终端给出的地址。关闭监听后，保存文件也只会看到第一次构建的页面。只想关这一次，用 `tola dev --watch=false`。

`tola dev` 按每个 hook 的 `dev` 设置运行。要预览一次生产构建，用 `tola preview`。两个命令都读取 `[server]`。

### 调试开发中的 hook

如果改数据文件也应重建，就把路径加到 hook 的 `rerun-on`。Tola 会自动监听 `nu` 指定的脚本。要让配置了 `dev = "reuse"` 的 `generate-outputs` hook 每次重建都运行，可以用 `tola dev --no-reuse`。各阶段的选项见 [`[hooks]`](tola-help://config/hooks)。
