# `@tola/code`

代码语法、主题和共享的代码样式表

## `code-syntaxes`

按语言索引的附加语法。可以给 `raw.syntaxes` 传入单份语法，或在代码块前用
`#set raw(syntaxes: code-syntaxes.values())` 启用全部语法。

## `code-language`

把源码标签映射到 Typst 的内置语法名，用于构造 `raw(..., lang: ...)`。
例如 `code-language("sbcl")` 和 `code-language("ccl")` 返回 `"lisp"`。

Example - 为 SBCL 源码使用 Lisp 语法：

```typst
#import "@tola/code:0.0.0": code-language
#document("index.html")[
  #raw("(defun twice (x) (* x 2))", lang: code-language("sbcl"))
]
```

### `language`

要查找的源码标签。

## `code-themes`

本包自带的主题。

`code-themes.tokyo-night` 是主题文件的路径，传给 `raw.theme` 或 `render-code(dark-theme: …)`。

Example - 检查主题名是否为键：

```typst
#import "@tola/code:0.0.0": code-themes
#assert("tokyo-night" in code-themes)
#assert("tokyo-night-dark" not in code-themes)
```

## `render-code`

把 raw 元素渲染成代码，保留 Typst 自己的高亮。

用 show rule 应用：`#show raw: render-code`。Typst 先准备元素的行与高亮，再把它交给 show rule。把代码围栏或 `#raw(...)` 放进文档；直接调用 `render-code(raw(...))` 会跳过准备阶段，因此报错。HTML 导出之外，原样返回该元素。

每个块长什么样，由 `raw.theme` 在 Typst 高亮之前决定：`auto` 用 Typst 默认主题，路径或主题字节用该主题，`none` 关闭高亮。主题写在代码块处，用 `#set raw(theme: …)` 或 `#show raw.where(…)` 规则。

把 `code-stylesheet()` 加载到文档 head 中，让浏览器应用渲染后的 token 样式。用 `raw.theme` 设置主要外观，用 `dark-theme` 设置深色外观。

Example - 在完整页面中用浅色与深色主题渲染代码：

```typst
#import "@tola/code:0.0.0": code-themes, code-stylesheet, render-code
#document("index.html", title: [Code])[
  #html.elem("html", attrs: (lang: "en", "data-theme": "dark"))[
    #html.head[
      #html.title("Code")
      #code-stylesheet()
    ]
    #html.body[
      #title()
      #set raw(theme: code-themes.github)
      #show raw: render-code.with(dark-theme: code-themes.github-dark)
      #raw("let answer = 42;", lang: "rust", block: true)
    ]
  ]
]
```

### `raw`

要渲染的 raw 元素。

### `attrs`

Tola 渲染代码所用元素上的 HTML 属性，例如 `class`、`id`、`data-*`。`class` 和 `style` 在 helper 所设值上扩展；`data-lang` 会被拒绝，语言 tag 已经设置了它。

### `dark-theme`

站点深色状态用的主题：`code-themes` 里的值、`path("/…")` 值，或主题字节。站点根元素带 `data-theme="dark"` 时生效。文件在构建时读取，主题读不到会让构建失败；普通字符串会被拒绝，因为它会解析到本包内部。

## `code-stylesheet-url`

代码样式表挂载到的浏览器 URL。

样式表给 `render-code` 渲染出的代码上色。需要给 head 条目加自己的属性时用这个 URL；`code-stylesheet` 返回现成的条目。每次构建都会在 `_tola/` 下发布共享样式表，这个 URL 已经包含 `site.base-path`。

Example - 给 head 条目加上自己的属性：

```typst
#import "@tola/code:0.0.0": code-stylesheet-url
#let entry = html.link(rel: "stylesheet", href: code-stylesheet-url(), media: "screen")
#document("index.html")[
  #html.html[
    #html.head(entry)
    #html.body[Code]
  ]
]
```

## `code-stylesheet`

加载代码样式表的 head 条目。

直接放进 head。条目需要自己的属性时，改用 `code-stylesheet-url()` 构造。

Example - 在文档 head 中加载样式表：

```typst
#import "@tola/code:0.0.0": code-stylesheet
#document("index.html")[
  #html.html[
    #html.head(code-stylesheet())
    #html.body[Code]
  ]
]
```
