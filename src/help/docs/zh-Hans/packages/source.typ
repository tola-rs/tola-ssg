#show: package(
  summary: [发现 Typst 内容文件、读取当前源文件信息，以及声明与校验文章元数据。],
  related: ("@tola/address", "@tola/schema", "[build]"),
)

站点入口决定将哪些发现的源文件发布为页面，并自行定义 `title`、`draft`、`permalink` 等元数据字段的含义。

#export(
  "all-sources",
  summary: [列出 `build.content-dir` 下发现的全部 Typst 内容文件，按相对路径排序。],
  related: ("current-source", "tola-meta", "parse-sources", "@tola/address#join-route"),
)[
  扫描 `build.content-dir` 中以小写 `.typ` 结尾的文件，跳过隐藏文件、符号链接与站点入口。按日期或其他字段排序请在返回后自行排序。

  #table(
    columns: 2,
    table.header([记录字段], [说明]),
    [`id`、`path`], [相对于 `build.content-dir` 的路径（含 `.typ`，如 `"posts/deep.typ"`）。文件改名或移动时两者都会变化；feed 条目请另存固定 `id`。],
    [`file`], [以站点根目录为准的 Typst `path`，可直接传给 `include` 或 `read`。],
    [`filename`], [最后一段文件名（如 `"deep.typ"`）。],
    [`route-segments`], [由路径推导的默认路由段（首页为 `()`，`about.typ` 与 `about/index.typ` 均为 `("about",)`）。],
    [`meta`], [通过 `tola-meta` 声明的元数据字典，在 `site.typ` 中跨页汇总读取。],
  )

  运行 `tola inspect sources` 可不求值元数据直接列出发现的文件；运行 `tola query` 可将元数据导出为 JSON。
]

#export(
  "current-source",
  summary: [返回当前调用所在内容源文件的 `(id, file, path, filename, route-segments)`。],
  related: ("all-sources", "tola-meta", "@tola/document#current-document"),
)[
  无需 `context`。请在文章内调用再把结果传给模板——直接在位于 `build.content-dir` 外的模板里调用会报错。要查询文章的发布输出路径与路由，请用 #ref("@tola/document#current-document")。
]

#export(
  "tola-meta",
  summary: [声明当前内容源文件的元数据字典。],
  params: (
    payload: [为该内容文件声明的元数据字典。],
  ),
  related: ("all-sources", "parse-sources"),
)[
  每个内容文件最多调用一次，写在文件前部或通过导入的封装函数调用。放在 `context` 或带 selector 的 show rule 内部不会声明源文件元数据；顶层 `#show:` 在正文求值之后运行，正文出错可能导致声明未执行。即使站点入口随后把草稿筛掉，内容文件中的语法或运行时错误仍会使构建失败。

  返回值是一个不可见的 `<tola-meta>` 标记（可用 `query(<tola-meta>)` 查询），但元数据在调用时即被记录，与返回值是否插入文档内容无关。

  #example(title: [将 `tola-meta` 封装为具名参数辅助函数], check: "default")[
  ```typst
  #import "@tola/source:0.0.0": tola-meta

  #let post(..fields) = tola-meta(fields.named())
  ```
  ]
]

#export(
  "parse-sources",
  summary: [用 `@tola/schema` 规则校验源文件元数据，并返回附带解析后 `meta` 的记录数组。],
  params: (
    sources: [来自 `all-sources()` 或其子集的源文件记录数组。],
    schema: [`@tola/schema` 规则，或按每条记录返回对应规则的函数 `(source) => schema`。],
  ),
  related: ("@tola/schema#schema", "all-sources", "tola-meta"),
)[
  未调用 `tola-meta` 的文件按空字典校验。返回数组保持原有顺序与路径；校验失败时，诊断会直接指向出错文件及其元数据位置。

  #example(title: [校验元数据、跳过草稿并发布页面], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": join-route, output-of, slugify
  #import "@tola/schema:0.0.0": non-empty, optional, schema, trim
  #import "@tola/source:0.0.0": all-sources, parse-sources

  #let page-schema = schema((
    title: non-empty(trim(str)),
    draft: optional(bool, default: false),
  ), unknown: "keep")

  #let pages = parse-sources(all-sources(), page-schema).filter(s => not s.meta.draft)
  #for page in pages {
    let output = output-of(join-route(page.route-segments.map(slugify)))
    document(output, format: "html", title: page.meta.title)[#include page.file]
  }
  ```
  ]
]
