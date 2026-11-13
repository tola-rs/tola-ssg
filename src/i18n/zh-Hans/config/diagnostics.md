# `[diagnostics]`

限制终端一次显示多少条错误和警告。

## 配置项

### `max_errors`

终端最多显示几条错误。默认 3 条，设为 `0` 隐藏错误。

### `max_warnings`

终端最多显示几条警告。默认 3 条，设为 `0` 隐藏警告。

## 用法

例如，只显示错误，最多 10 条：

```toml
[diagnostics]
max_errors = 10
max_warnings = 0
```

这两项只限制终端显示的条数。有错误时，构建仍会失败。

### 查看完整报告

把完整报告保存到 `check.json`，再在终端打开：

```sh
tola check --json --pretty > check.json
tola nu --stdin @tola-scripts/diagnostic render < check.json
```

完整报告包含终端省略的错误和警告。字段说明见 [`@tola-scripts/diagnostic`](tola-help://scripts/diagnostic)。
