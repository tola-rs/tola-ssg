# `@tola/document`

查看当前文档、列出标题和查找 Typst 链接。

在 `document(…)` 正文的 `context` 里调用这些函数。要查某个 Typst 源文件的路径或元数据，用 `@tola/source`。

## `current-document`

返回当前文档。在 `document(…)` 正文的 `context` 里调用。

- `output`：输出文件路径，例如 `notes/index.html`。
- `route`：站内路由，例如 `/notes/`，不带部署前缀。
- `location`：文档在 Typst 中的位置，可传给 `link` 或 `query`。

示例：读取当前文档的输出路径和路由：

```typst
#import "@tola/document:0.0.0": current-document
#document("index.html")[
  #context {
    let document = current-document()
    assert.eq(document.output, "index.html")
    assert.eq(document.route, "/")
  }
]
```

示例：只查询当前文档的标题：

```typst
#import "@tola/document:0.0.0": current-document
#document("index.html")[
  = Intro
  #context {
    let sections = query(selector(heading).within(current-document().location))
    assert.eq(sections.len(), 1)
  }
]
```

## `references`

按文档顺序查找 Typst 的 `link` 和 `ref`。例如，可以找出哪些页面链接到了当前页。

在 `context` 中调用。结果包含 show rule 执行后仍是 Typst `link` 或 `ref` 的最外层链接。链接被换成普通文字或手写 HTML 时会跳过。多处出现的同一个链接会各占一条记录。

| 参数 | 选取 |
| --- | --- |
| `from` | 选中元素所在的文档 |
| `from-within` | 只查在该范围内出现的链接 |
| `to` | 目标文档、资源或具体的 Typst 元素 |
| `to-within` | 只匹配指向该范围内 Typst 元素的链接 |

四个参数都可以传 label、location、可定位的元素函数或 selector。默认 `none`，不按该条件筛选。用 `selector(...).or(...)` 或 `.and(...)` 组合选择。label 会选中所有匹配元素，找不到时返回空数组。同一个链接只记录一次。

`from` 找到所选元素所在的文档，选中资源或文档外的内容会报错。`to` 按输出路径匹配文档和资源，按 location 匹配其他 Typst 元素。要选当前文档，在正文里传 `from: auto` 或 `to: auto`。

`from-within` 和 `to-within` 只查所选元素里面的内容。标题的 label 只选中标题本身。要选中整个章节，给包住正文的元素加 label，例如 `html.section`。有多个筛选条件时，链接必须全部满足。

`ref` 的目标是 label，`link` 的目标是链接地址。查询以 show rule 执行后的元素为准。Typst 引用必须指向唯一目标。带协议和主机的 URL 会标为 `external`，即使主机是本站。想同时查外链和找不到目标的链接，省略目标条件。

| 字段 | 含义 |
| --- | --- |
| `element` | show rule 执行后的 `link` 或 `ref` 元素 |
| `document` | 来源文档的 `output`、`route` 与 `location` |
| `destination` | 该元素的链接目标 |
| `resolution` | `found`、`external` 或 `unresolved` |
| `reason` | 未找到目标的原因。仅当 `resolution` 为 `unresolved` 时提供 |
| `target` | 查到的目标信息。无可用信息时为 `none` |

目标字典有 `kind`、`output`、`route`、`query`、`fragment` 和 `location`。取不到的字段是 `none`。`kind` 为 `output` 时，指向 Typst 文档或资源。为 `element` 时，指向 Typst 元素。为 `url` 时，目标来自 URL。通过 URL 写的链接仍能用 `to` 按文档或资源筛选。按元素筛选或使用 `to-within` 时，需要目标有 Typst location。

URL 中的 `fragment` 会解码，Typst 生成的锚点按原样返回。链接指向整个文档或资源时，`fragment` 为 `none`。要检查最终 HTML 中的链接和锚点，包括手写 HTML，用 `tola check`。如果用查询结果生成新链接，把它们放在 `from-within` 所选范围外。

用 `reason.tag` 判断原因，显示问题时用 `reason.message`，用 `reason.help` 提供修复建议。

| tag | 含义 |
| --- | --- |
| `outside-mount` | 目标落在 `site.base-path` 之外 |
| `invalid-destination` | 目标不是本站能读取的 URL |
| `no-such-target` | 本次构建没有与目标匹配的输出文件 |
| `positional-destination` | 链接指向页内坐标。改用 URL、label 或元素的 location |
| `label-has-no-location` | 该 label 对应的内容在本次构建中没有可链接的位置 |
| `ambiguous-label` | 多个元素共用一个 label |

示例：查找哪些文档链接到了当前文档：

```typst
#import "@tola/address:0.0.0": output-url
#import "@tola/document:0.0.0": references

#let guide-output = "guide/index.html"
#let install-output = "guide/install/index.html"

#document(guide-output, title: [Guide])[
  #html.main[#link(output-url(install-output))[Install]] <body>
]

#document(install-output, title: [Install])[
  #html.main[Install] <body>
  #context {
    let incoming = references(to: auto, from-within: <body>)
    assert.eq(incoming.len(), 1)
    assert.eq(incoming.first().document.output, guide-output)
    for reference in incoming [
      #link(reference.document.location)[From #reference.document.route]
    ]
  }
]
```

示例：查找指向指定标题的引用：

```typst
#import "@tola/address:0.0.0": output-url
#import "@tola/document:0.0.0": references

#let output = "guide/install/index.html"

#document(output, title: [Install])[
  #link(<intro>)[Jump to the introduction]
  #link(output-url(output))[Whole page]
  = Introduction <intro>
  #context {
    let intro = query(<intro>).first().location()
    assert.eq(references(to: intro).len(), 1)
    assert.eq(references(to: intro).first().target.fragment, "intro")
  }
]
```

示例：查找从指定文档发出的链接：

```typst
#import "@tola/document:0.0.0": references

#document("guide/index.html", title: [Guide])[
  #html.main[#link("https://typst.app/")[Typst]] <body>
  #context {
    let written = references(from: auto, from-within: <body>)
    assert.eq(written.len(), 1)
    assert.eq(written.first().resolution, "external")
  }
]
```

示例：查找指向另一篇文档中某个章节的链接：

```typst
#import "@tola/document:0.0.0": references
#document("a.html")[
  #html.main[
    #link(<detail>)[Detail]
    #link(<chapter>)[Chapter]
  ] <a-body>
  #context {
    let found = references(
      from: <a>, from-within: <a-body>,
      to: <b>, to-within: <chapter>,
    )
    assert.eq(found.len(), 1)
  }
] <a>
#document("b.html")[
  #html.section[
    = Detail <detail>
  ] <chapter>
] <b>
```

示例：按多个目标或某个章节筛选：

```typst
#import "@tola/document:0.0.0": references
#document("index.html")[
  #link(<first>)[First]
  #link(<second>)[Second]
  #context {
    let targets = selector(<first>).or(<second>)
    assert.eq(references(from: auto, to: targets).len(), 2)
    let topics = selector(heading).within(<topics>)
    assert.eq(references(from: auto, to: topics).len(), 2)
  }
]
#document("topics.html")[
  #html.section[
    = First <first>
    = Second <second>
  ] <topics>
]
```

### `from`

只查所选元素所在文档里的链接。默认 `none`，查所有文档。

### `from-within`

只查出现在所选元素里的链接。默认 `none`，不限制出现位置。

### `to`

只查指向所选文档、资源或元素的链接。默认 `none`，不限制目标。

### `to-within`

只查指向所选元素内部的链接。默认 `none`，不限制目标所在范围。

## `headings`

当前文档的标题，按文档顺序。

每个标题都会列出，包括 `outlined: false` 排除在 Typst `outline()` 之外的标题。没有任何标题的文档返回空数组。

| 字段 | 含义 |
| --- | --- |
| `level` | 加上 `offset` 后的标题层级 |
| `nesting` | 标题在大纲中的层级，不参与大纲时为 `none` |
| `number` | 标题编号，可以是模式生成的字符串、回调返回值或 `none` |
| `body` | show rule 生效前的标题源内容 |
| `text` | show rule 生效前的标题纯文本 |
| `label` | 标题的标签，没有时为 `none` |
| `location` | 标题的位置，可传给 `link` |
| `outlined` | 标题是否参与大纲 |

要让目录和 Typst 的 `outline()` 一致，按 `outlined` 筛选。

例如，设了 `set heading(offset: 2)` 后，写成 `==` 的标题的 `level` 为 4。

`headings()` 只查当前文档。`query(selector(heading))` 默认查整个 Bundle。要把其他查询也限定在当前文档，用 `query(selector(…).within(current-document().location))`。

在 `context` 中调用。把 `location` 传给 `link()` 后，Typst 会为标题生成锚点。带 label 的标题用 label 作锚点，其余用 `loc-N`。没人链接到的标题，在导出的 HTML 里没有锚点。

示例：为当前文档生成目录：

将下面的代码保存为新站点的 `site.typ`，然后运行 `tola build`。首页会链接到 Introduction 和 Details。三级标题和 `outlined: false` 的标题会被跳过。

```typ
#import "@tola/document:0.0.0": headings
#document("index.html")[
  #context {
    let sections = headings(depth: 2).filter(section => section.outlined)
    assert.eq(sections.map(section => section.text), ("Introduction", "Details"))
    for section in sections [#link(section.location, section.text)]
  }
  = Introduction
  == Details
  === Deep section
  #heading(outlined: false)[Hidden section]
]
#document("other.html")[= Another document]
```

示例：查看大纲中的 `nesting` 和 `label`：

```typst
#import "@tola/document:0.0.0": headings
#document("index.html")[
  = Intro <intro>
  #heading(level: 3, outlined: false)[Aside]
  == Detail
  #context {
    let sections = headings()
    assert.eq(sections.map(section => section.label), (<intro>, none, none))
    assert.eq(sections.map(section => section.nesting), (1, none, 2))
  }
]
```

### `depth`

省略或传 `none` 时列出所有标题。传正整数时，只保留实际 `level` 不超过该值的标题。例如，`==` 标题在 `offset: 1` 下是三级，`depth: 2` 会略过它。

## `plain-text`

从字符串或 Typst 内容中提取纯文本。

文本、符号、空格、代码和弯引号按原样读取。格式、链接和数学元素只取里面的文本，metadata 跳过。返回的是原文，不执行 show rule 或排版。

遇到无法提取文本的空元素时会跳过，并提示对应位置。

示例：从字符串与内容中读取文本：

```typst
#import "@tola/document:0.0.0": plain-text
#assert.eq(plain-text("Guide"), "Guide")
#assert.eq(plain-text([Hello, *world*!]), "Hello, world!")
```

示例：从 HTML 图片元素中提取文本：

```typst
#import "@tola/document:0.0.0": plain-text
#let image = html.img(src: "/cover.png", alt: "Cover")
#assert.eq(plain-text(image), "")
```

### `value`

要读取的字符串或 Typst 内容。
