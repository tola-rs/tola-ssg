# `@tola/code`

给 HTML 中的代码加语法高亮，选择浅色和深色主题。

## `code-syntaxes`

Tola 自带的附加语法，按语言标签索引。如果 `raw.syntaxes` 中有同名语法，优先用你提供的版本。

## `code-language`

把语言别名转成 Typst 能识别的语法名，供 `raw(..., lang: ...)` 使用。
例如 `code-language("sbcl")` 和 `code-language("ccl")` 返回 `"lisp"`。


用下面的代码替换新站点的 `site.typ`，从站点根目录运行 `tola build`。`public/index.html` 中的代码块会使用 Lisp 语法。

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


```typst
#import "@tola/code:0.0.0": code-themes
#assert("tokyo-night" in code-themes)
#assert("tokyo-night-dark" not in code-themes)
```

## `render-code`

把 Typst 已经高亮的代码输出为 HTML。

写 `#show raw: render-code`，再用代码块或 `#raw(...)` 写代码。直接调用 `render-code(raw(...))` 会报错，因为此时还没有高亮结果。导出非 HTML 文档时，返回原来的 `raw` 元素。

用 `#set raw(theme: …)` 或 `#show raw.where(…)` 指定浅色主题。`auto` 用 Typst 的默认主题，也可以传主题文件的路径或字节。传 `none` 关闭浅色和深色高亮。

把 `code-stylesheet()` 放进 HTML 的 `<head>`，浏览器才会给代码上色。深色主题用 `dark-theme` 设置。


将下面的代码保存为新站点的 `site.typ`，运行 `tola dev` 并打开打印的地址。页面会用深色主题高亮 Rust 代码。将 `data-theme` 改成 `"light"` 可查看浅色主题。

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

给代码元素添加 HTML 属性，例如 `(class: "example",)`。省略时沿用生成的属性。`class` 和 `style` 会追加到原值。`data-lang` 由代码语言设置。

### `dark-theme`

根元素带 `data-theme="dark"` 时使用的主题。省略时两种模式沿用同一套高亮。可传 `code-themes` 里的值、Typst 的 `path` 或主题字节。主题文件在构建时读取。

## `code-stylesheet-url`

共享代码样式表的 URL。

如果想给样式表的 `<link>` 添加属性，用返回的 URL 自己写标签。链接已带 `site.base-path`，每次构建都会发布这份样式表。


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

生成加载代码样式表的 `<link>`，放在 HTML 的 `<head>` 中。

如果要给标签添加属性，用 `code-stylesheet-url()` 取 URL。


```typst
#import "@tola/code:0.0.0": code-stylesheet
#document("index.html")[
  #html.html[
    #html.head(code-stylesheet())
    #html.body[Code]
  ]
]
```
