#show: config(
  summary: [配置 `tola fmt` 与 `tola lsp` 共用的 Typst 代码格式化选项。],
)

运行 `tola fmt --check` 在 CI 中检查格式，运行 `tola fmt` 就地格式化。在站点目录之外调用时，`tola fmt` 使用默认设置格式化当前目录下的 `.typ` 文件。

#example(title: [将折行宽度设为 88 列])[
  ```toml
  [formatter]
  indent = 2
  print-width = 88
  prose-wrap = "none"
  ```
]

#key("indent")[
  每层缩进使用的空格数。
]

#key("print-width")[
  目标折行宽度（列数）。小于 `20` 时按 `20` 处理。
]

#key("prose-wrap")[
  正文段落折行方式：`"none"` 保留原有换行，`"fill"` 按 `print-width` 重新折行，`"sentence"` 每句单独一行。
]
