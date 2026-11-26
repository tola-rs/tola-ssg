#show: script(
  summary: [在终端中渲染 JSON 诊断报告，或在 Nu hook 中提交错误与警告。],
)

#command(
  "main",
  summary: [列出子命令。在 Nu hook 中请通过 `use tola diagnostic` 直接提交诊断。],
)[
  在 Nu 构建 hook 或模块命令中，调用一次 `diagnostic submit` 提交全部问题（无问题时传入 `[]`）。提交列表中只要包含 `error` 级别诊断，hook 即视为失败。独立脚本中可用 `diagnostic render` 直接输出到终端。

  #example(title: [在 Nu hook 中报告警告])[
    ```nu
    use tola diagnostic

    let issues = if (open --raw content/index.typ | str contains "TODO") {
      [(diagnostic make "首页仍包含 TODO 标记。" --code draft-todo)]
    } else {
      []
    }
    $issues | diagnostic submit
    ```
  ]
]

#command(
  "render",
  summary: [从标准输入读取 JSON 诊断报告，并以终端样式打印。],
)[
  调用时将 `--stdin` 放在脚本名之前。输入可以是诊断对象数组，也可以是包含 `diagnostics` 数组的对象（例如 `tola check --json` 的输出）。

  #table(
    columns: 2,
    table.header([字段], [说明]),
    [`severity`], [必填。可选 `error`、`warning`、`info` 或 `hint`。],
    [`message`], [必填。对问题的简明描述。],
    [`code`], [可选。规则标识，例如 `missing-alt`。],
    [`location`], [可选。位置对象，包含 `path`（绝对路径或相对于当前工作目录），以及可选的 1-based `line` 与 `column`。],
    [`hints`], [可选。简短的修复建议列表。],
    [`help`], [可选。结构化修复项列表，每项包含 `message` 与可选 `location`。],
    [`notes`], [可选。补充背景说明列表。],
    [`related`], [可选。关联信息列表，每项包含 `message` 与可选 `location`。],
  )

  #example(title: [渲染 `tola check` 的 JSON 输出])[
    ```sh
    tola check --json | tola nu --stdin @tola-scripts/diagnostic render
    ```
  ]

  #example(title: [自定义 JSON 诊断结构])[
    ```json
    [
      {
        "severity": "warning",
        "message": "封面图缺少替代文本。",
        "code": "missing-alt",
        "location": { "path": "content/index.typ", "line": 1 },
        "hints": ["为图片补充描述性的 alt 属性。"]
      }
    ]
    ```
  ]
]
