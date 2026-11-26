#show: package(
  summary: [在 HTML 导出中为代码块提供语法高亮与浅色/深色主题支持。],
)

#export(
  "code-syntaxes",
  summary: [Tola 内置附加语法定义字典，按语言标签索引。],
  related: ("code-language", "render-code"),
)[
  若 `raw.syntaxes` 中声明了同名语法，优先使用站点提供的版本。
]

#export(
  "code-language",
  summary: [将语言别名规范化为 Typst `raw(..., lang: ...)` 可识别的语法名。],
  params: (
    language: [源语言标签或别名（例如 `"sbcl"` → `"lisp"`）。],
  ),
  related: ("code-syntaxes", "render-code"),
)

#export(
  "code-themes",
  summary: [内置 `.tmTheme` 主题路径字典，供 `raw.theme` 与 `render-code(dark-theme: ...)` 使用。],
  related: ("render-code", "code-stylesheet"),
)[
  直接将字典成员（如 `code-themes.github`、`code-themes.tokyo-night`）传给 `#set raw(theme: ...)` 或 `render-code(dark-theme: ...)`。
]

#export(
  "render-code",
  summary: [将 Typst 已高亮的 `raw` 元素渲染为带主题样式的 HTML，支持浅/深色双主题。],
  params: (
    raw: [由 `#show raw:` 规则传入的 `raw` 元素。],
    dark-theme: [当根 HTML 元素带 `data-theme="dark"` 时生效的主题（可传 `code-themes.*`、`path` 或 `bytes`）；省略时浅/深色沿用同一主题。],
    attrs: [附加到代码元素的 HTML 属性字典（如 `(class: "example",)`）。`class` 与 `style` 会追加到原有值；`data-lang` 由代码语言自动设置。],
  ),
  related: ("code-themes", "code-stylesheet", "code-stylesheet-url"),
)[
  通过 `#show raw: render-code` 应用（直接调用 `render-code(raw(...))` 会报错，因为尚未经过 Typst 高亮）。非 HTML 导出时原样返回 `raw` 元素。

  浅色主题由 `#set raw(theme: ...)` 指定（`auto` 用 Typst 默认主题，`none` 关闭浅色与深色高亮）。请将 `code-stylesheet()` 放入 HTML `<head>`，浏览器才会加载着色样式。

  #example(title: [为 HTML 代码启用浅/深色主题高亮], check: "default")[
  ```typst
  #import "@tola/code:0.0.0": code-stylesheet, code-themes, render-code

  #document("index.html", title: [Code])[
    #html.elem("html", attrs: (lang: "en", "data-theme": "dark"))[
      #html.head[
        #html.title("Code")
        #code-stylesheet()
      ]
      #html.body[
        #set raw(theme: code-themes.github)
        #show raw: render-code.with(dark-theme: code-themes.github-dark)
        #raw("let answer = 42;", lang: "rust", block: true)
      ]
    ]
  ]
  ```
  ]
]

#export(
  "code-stylesheet-url",
  summary: [返回共享代码高亮样式表的 URL（已包含 `site.base-path`）。],
  related: ("code-stylesheet", "render-code"),
)[
  适合需要为 `<link>` 附加自定义属性（如 `media`）的场景；被引用时该样式表会自动发布。
]

#export(
  "code-stylesheet",
  summary: [返回用于放入 `<head>` 的代码高亮样式表 `<link rel="stylesheet">` 元素。],
  related: ("code-stylesheet-url", "render-code"),
)
