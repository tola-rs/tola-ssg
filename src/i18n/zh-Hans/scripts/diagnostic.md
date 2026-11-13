# `@tola-scripts/diagnostic`

在终端查看 JSON 报告，或让 Nu hook 向构建报告错误和警告。

## `main`

运行 `tola nu @tola-scripts/diagnostic` 查看子命令。

在 Nu hook 中，用 `diagnostic submit` 一次提交所有问题。没有问题时传 `[]`。普通 hook 可以省略 `location`。独立脚本可用 `diagnostic render` 在终端显示问题。

示例：让 Nu hook 报告未删掉的 TODO：

在用 `tola init` 创建的站点中，给 `content/index.typ` 写一行 `TODO`。然后新建 `tools/`，把下面的代码保存为 `tools/check-draft.nu`：

```nu
use tola diagnostic

let issues = if (open --raw content/index.typ | str contains "TODO") {
  [
    (diagnostic make "请清理首页中的 TODO。" --code draft-todo)
  ]
} else {
  []
}
$issues | diagnostic submit
```

在 `tola.toml` 中加入这个 hook：

```toml
[[hooks.before-build]]
name = "check-draft"
nu = "tools/check-draft.nu"
```

运行 `tola check`。如果首页源文件里还有 `TODO`，这个 hook 会发出警告。给 `diagnostic make` 加上 `--severity error`，就能在发现 `TODO` 时让检查失败。

## `render`

从标准输入读取 JSON 报告。把 `--stdin` 放在脚本名前。可直接传入问题数组，也可传入带 `diagnostics` 数组的对象。

**诊断字段**

- `severity`：必填。可用 `error`、`warning`、`info`、`hint`。
- `message`：必填。说明发现的问题。
- `code`：可选。规则名，例如 `missing-alt`。
- `location`：问题所在位置。填写时要提供 `path`，可用绝对路径或相对于命令运行目录的路径。`line` 和 `column` 可省略，均从 1 开始计数。
- `hints`：可选。修复建议，值为字符串数组。
- `help`：可选。每条修复建议填写 `message`，也可附上 `location`。
- `notes`：可选。补充说明，值为字符串数组。
- `related`：可选。每条补充说明填写 `message`，也可附上 `location`。

示例：显示文件中的诊断报告：

在已有 `content/index.typ` 的站点中，把下面的内容保存为 `issues.json`，与 `tola.toml` 放在一起：

```json
[
  {
    "severity": "warning",
    "message": "封面图片缺少替代文本。",
    "code": "missing-alt",
    "location": {"path": "content/index.typ", "line": 1},
    "hints": ["在 alt 文本中描述图片内容。"]
  }
]
```

从站点根目录运行：

```sh
tola nu --stdin @tola-scripts/diagnostic render < issues.json
```

终端会显示一条 `missing-alt` 警告，标出 `content/index.typ` 第 1 行，并附上修复建议。

示例：显示 `tola check` 的报告：

```sh
tola check --json > check.json
tola nu --stdin @tola-scripts/diagnostic render < check.json
```
