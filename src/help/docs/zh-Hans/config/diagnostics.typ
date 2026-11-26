#show: config(
  summary: [限制单次运行在终端中显示的错误与警告条数。],
  related: ("@tola-scripts/diagnostic",),
)

这两项仅控制终端输出条数；只要存在错误，构建仍会失败。如需查看未被截断的完整报告，可将 `tola check --json` 通过管道交给 #ref("@tola-scripts/diagnostic#render")。

#example(title: [最多显示 10 条错误并隐藏终端警告])[
  ```toml
  [diagnostics]
  max_errors = 10
  max_warnings = 0
  ```
]

#key("max_errors")[
  终端最多显示的错误条数；设为 `0` 时不在终端打印错误。
]

#key("max_warnings")[
  终端最多显示的警告条数；设为 `0` 时不在终端打印警告。
]
