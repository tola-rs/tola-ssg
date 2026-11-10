# `[diagnostics]`

更改终端输出时显示诊断的数量上限。

## 配置项

### `max_errors`

每批终端输出最多显示几条错误，默认 3。

### `max_warnings`

每批终端输出最多显示几条警告，默认 3。

## 用法

`max_errors` 与 `max_warnings` 限制一批终端输出最多显示几条诊断。错误先于警告输出，每个严重级别各按自己的上限计数，每次开发重建都会开始新的一批。终端位置沿用 Typst CLI 的编号：行号从 1 开始，列号从 0 开始。

```toml
[diagnostics]
max_errors = 3
max_warnings = 3
```

把上限设为 `0`，可在终端隐藏对应的严重级别——这只是终端输出：`--log-file` 的完整记录与编辑器通过 LSP 拿到的诊断都不受影响。终端里显示的路径相对你运行命令的位置；编辑器按打开的文件与行列出诊断。超出上限的批次会在末尾说明隐藏了多少条：

```
3 errors not shown; increase `diagnostics.max_errors` to show more
```

`[diagnostics]` 并不会影响构建本身的结果。
