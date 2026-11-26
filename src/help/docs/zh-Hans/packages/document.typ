#show: package(
  summary: [读取当前文档信息、列出文档标题、查询跨文档引用及提取纯文本。],
  related: ("@tola/source", "@tola/address"),
)

`current-document`、`headings` 与 `references` 均在 `#document(...)` 正文的 `context` 中调用。建议按成员导入（或为包起别名），避免覆盖 Typst 内置的 `document(...)`。

#export(
  "current-document",
  summary: [在 `context` 中返回当前所在 `#document(...)` 的信息。],
  related: ("headings", "references", "@tola/source#current-source"),
)[
  返回包含以下字段的字典：
  - `output`：相对于发布目录的输出路径（例如 `"notes/index.html"`）。
  - `route`：不含部署前缀的站点根路由（例如 `"/notes/"`）。
  - `location`：当前文档的 Typst `location`，可传给 `link(...)` 或 `query(selector(...).within(current-document().location))`。

  #example(title: [将 Typst 查询限定在当前文档内], check: "default")[
  ```typst
  #import "@tola/document:0.0.0": current-document

  #document("index.html")[
    = 概览
    #context {
      let here = current-document()
      let local-headings = query(selector(heading).within(here.location))
      [路由：#here.route（共 #local-headings.len() 个标题）]
    }
  ]
  ```
  ]
]

#export(
  "references",
  summary: [按文档顺序查询整个 Bundle 中的 Typst `link` 与 `ref`（例如实现反向链接）。],
  params: (
    from: [仅查询所选元素所在文档中的链接（`auto` 表示当前文档，`none` 表示全部文档）。],
    from-within: [仅查询出现在所选祖先元素范围内的链接。],
    to: [仅查询指向所选文档、资源或元素的链接（`auto` 表示当前文档）。],
    to-within: [仅查询指向所选祖先范围内 Typst 元素的链接。],
  ),
  related: ("current-document", "headings", "@tola/address#output-url"),
)[
  在 `context` 中调用。四个筛选参数均接受 label、location、可定位元素函数、selector、`auto`（仅 `from`/`to`）或 `none`，多个条件需同时满足。结果仅包含 show rule 生效后仍为最外层 `link`/`ref` 的元素，不含手写 HTML 链接（完整 HTML 校验请运行 `tola check`）。

  #table(
    columns: 2,
    table.header([结果字段], [含义]),
    [`element`], [show rule 生效后的 `link` 或 `ref` 元素。],
    [`document`], [来源文档的 `(output, route, location)` 字典。],
    [`destination`], [元素上声明的链接目标。],
    [`resolution`], [`"found"`、`"external"` 或 `"unresolved"`。],
    [`reason`], [仅在 `"unresolved"` 时提供：`(tag, message, help)`（`outside-mount`、`invalid-destination`、`no-such-target`、`positional-destination`、`label-has-no-location`、`ambiguous-label`）。],
    [`target`], [目标字典 `(kind, output, route, query, fragment, location)`（`kind` 为 `"output"`、`"element"` 或 `"url"`），不可用时为 `none`。],
  )

  若根据查询结果动态渲染新链接，请将其置于 `from-within` 范围之外以保持查询结果稳定。

  #example(title: [列出链接到当前页面的反向链接], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": output-url
  #import "@tola/document:0.0.0": references

  #document("a.html", title: [A])[#link(output-url("b.html"))[查看 B]]
  #document("b.html", title: [B])[
    #context {
      let incoming = references(to: auto)
      for item in incoming [来自 #item.document.route 的引用]
    }
  ]
  ```
  ]
]

#export(
  "headings",
  summary: [按文档顺序列出当前文档的所有标题。],
  params: (
    depth: [可选正整数，限制包含 `heading.offset` 后的最大层级；省略或传 `none` 返回全部标题。],
  ),
  related: ("current-document", "references"),
)[
  在 `context` 中调用。列出当前文档内的全部标题，包括 `outlined: false` 的标题（如需与 Typst `outline()` 保持一致，按 `h.outlined` 过滤）。

  #table(
    columns: 2,
    table.header([字段], [含义]),
    [`level`], [计入 `heading.offset` 后的实际层级。],
    [`nesting`], [在大纲中的嵌套层级；不参与大纲时为 `none`。],
    [`number`], [编号结果，无编号时为 `none`。],
    [`body` / `text`], [show rule 生效前的标题内容与纯文本。],
    [`label`], [标题标签，无标签时为 `none`。],
    [`location`], [标题的 Typst `location`，传给 `link(...)` 时 HTML 导出会生成锚点 ID。],
    [`outlined`], [是否参与大纲。],
  )

  #example(title: [生成二级页内目录], check: "default")[
  ```typst
  #import "@tola/document:0.0.0": headings

  #document("index.html")[
    #context {
      let toc = headings(depth: 2).filter(h => h.outlined)
      for h in toc [#link(h.location, h.body) ]
    }
    = 简介
    == 细节
  ]
  ```
  ]
]

#export(
  "plain-text",
  summary: [从字符串或 Typst 内容提取纯文本，不执行排版与 show rule。],
  params: (
    value: [待提取文本的字符串或 Typst 内容。],
  ),
)[
  保留文本、符号、空格、代码与弯引号，递归展开格式、链接与数学元素，跳过 metadata 与无法提取文本的空元素。

  #example(title: [提取富文本内容中的纯文本], check: "default")[
  ```typst
  #import "@tola/document:0.0.0": plain-text

  #let text = plain-text([Hello, *world*!])
  ```
  ]
]
