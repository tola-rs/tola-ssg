#show: config(
  summary: [配置 Typst 编译器与 SVG 光栅化使用的字体搜索目录。],
  related: ("[assets]", "@tola-scripts/image#resize"),
)

这里的字体仅用于 Typst 编译（如 PDF 导出、SVG 帧）以及站点 hook 或 #ref("@tola-scripts/image#resize") 渲染 SVG 文字。浏览器网页字体请通过 #ref("[assets]") 发布并用 CSS `@font-face` 加载。

Tola 先搜索 `paths`，再按 `modules` 列出的顺序搜索模块字体；同等匹配条件下，先搜到的字体优先。运行 `tola inspect fonts`（或 `--json`）查看可用字体族名称与来源。

- `--pure` 仍可使用 Tola 内置字体，但拒绝 `system = true` 与 `--font-path`。
- 命令行 `--font-path DIR` 可为本次运行追加最高优先级的字体目录，`--ignore-system-fonts` 可临时跳过系统字体。
- `tola vendor` 不会复制编译器字体，请将项目字体随仓库放在 `paths` 目录内。

#example(title: [使用项目与模块字体，跳过系统字体])[
  ```toml
  [typst.fonts]
  paths = ["static/typst-fonts"]
  modules = [
    "theme",
    { module = "cjk", rename = { "IBM Plex Sans" = "CJK Plex Sans" } },
  ]
  system = false
  ```
]

#key("paths")[
  相对于站点根目录的字体目录列表（递归扫描子目录）。
]

#key("modules")[
  按顺序加载这些模块在 `tola-module.toml` `[typst] fonts` 中声明的字体。可填模块别名字符串，或 `{ module = "别名", rename = { "原字体族" = "新字体族" } }`（不区分大小写匹配）将该字体族的所有字重与样式登记到新名称下。
]

#key("system")[
  编译时是否搜索操作系统字体。`--pure` 模式下设为 `true` 会报错。
]
