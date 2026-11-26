#show: package(
  summary: [在 Typst 求值期间向站点作者报告自定义错误、警告、信息与提示。],
  related: ("@tola-scripts/diagnostic", "[diagnostics]"),
)

#export(
  "report",
  summary: [记录一条诊断并继续求值，便于一次收集多个问题。],
  params: (
    message: [面向站点作者的清晰问题描述。],
    severity: [诊断级别：`"error"`、`"warning"`（默认）、`"info"` 或 `"hint"`。],
    code: [可选规则标识，由站点或模块自行命名（例如 `"site.missing-description"`）。],
    at: [源码位置：`auto`（默认）指向当前 `report(...)` 调用，传入 Typst 内容则指向该元素的源码位置，传 `none` 不附带位置。],
    hints: [可选的修复建议字符串数组。],
    related: [可选的关联位置数组，每项为 `(message: str, at: content | none)` 字典。],
  ),
)[
  任意 `"error"` 级别诊断会在求值结束后使 `tola check` 与 `tola build` 失败；`"warning"`、`"info"` 与 `"hint"` 不阻断构建。仅当无法继续求值时才使用 Typst 内置的 `panic(...)`。

  #example(title: [文章缺少简介时发出警告], check: "default")[
  ```typst
  #import "@tola/diagnostic:0.0.0": report

  #let check-page(title, description) = {
    if description == none {
      report(
        "页面 \"" + title + "\" 缺少 description。",
        severity: "warning",
        code: "site.missing-description",
        hints: ("在 `tola-meta` 中补充 `description` 字段。",),
      )
    }
  }
  ```
  ]
]
