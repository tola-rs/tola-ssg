#show: package(
  summary: [为站点文章生成 RSS、Atom 或 JSON Feed。],
  related: ("[site]", "@tola/sitemap", "@tola/schema#timestamp"),
)

#export(
  "feed",
  summary: [向站点 Bundle 输出一份 RSS、Atom 或 JSON Feed 文件。],
  params: (
    output: [相对于发布目录的输出路径。`auto` 按格式默认选用 `feed.xml`（RSS）、`atom.xml`（Atom）或 `feed.json`（JSON Feed）。],
    format: [订阅格式：`"rss"`（默认）、`"atom"` 或 `"json"`。],
    id: [Feed 标识。Atom 要求为绝对 URI；`auto` 默认由 `site.url` 生成。],
    title: [Feed 标题（不可为空白）；`auto` 取 `site.title`。],
    description: [Feed 简介；`auto` 取 `site.description`。],
    language: [BCP 47 语言标签；`auto` 取 `site.language.tag`。],
    authors: [作者 `(name:, email:, url:)` 字典数组；`auto` 取 `site.authors`。Atom 要求在 feed 级或每个条目上指定作者。],
    entries: [按发布顺序排列的条目字典数组，默认 `()`。],
  ),
)[
  需要在 #ref("[site]") 中配置 `origin` 以生成绝对 URL。

  = 条目字段

  每条记录必填 `target`（输出路径、`(output:, fragment:)`、label 或 location）与 `published`（按 UTC 解释的 Typst `datetime` 或带时区的 RFC 3339 字符串——使用 #ref("@tola/schema#timestamp") 时传入 `.rfc3339`）。可选字段：`id`（文章移动后请沿用原值；Atom 要求为绝对 URI）、`title`（默认取目标文档标题）、`updated`、`summary`、`content` 与 `authors`。

  #table(
    columns: 3,
    table.header([格式], [`summary`], [`content`]),
    [RSS], [`<description>`], [`<content:encoded>`],
    [Atom], [`<summary>`], [`<content>`（文本或 HTML）],
    [JSON Feed], [`summary`（纯文本）], [`content_text` 或 `content_html`],
  )

  = 条目 `content` 的写法

  - 字符串：按纯文本写入。
  - 简单 Typst 内容（文本、换行、强调、粗体、删除线与 URL 链接）：直接转为 HTML，不执行排版与 show rule。
  - 从已导出 HTML 提取正文：传入 `(document: output, id: "post-body")` 提取该元素及其后代（并保留 `<head>` 样式），省略 `id` 则取整个 `<body>`；元素内的相对 URL 会自动改写为绝对 URL。

  #example(title: [生成附带正文 HTML 的 RSS Feed], check: "site")[
  ```typst
  #import "@tola/feed:0.0.0": feed

  #let output = "notes/index.html"
  #document(output, title: [第一则笔记])[
    #html.html[
      #html.head(html.title("第一则笔记"))
      #html.body[
        #html.article(id: "post-body")[
          = 第一则笔记
          包含在订阅源中的完整正文。
        ]
      ]
    ]
  ]

  #feed(
    title: "Typst 笔记",
    entries: ((
      id: "urn:notes:first",
      target: output,
      published: datetime(year: 2026, month: 9, day: 1),
      summary: "订阅预览摘要。",
      content: (document: output, id: "post-body"),
    ),),
  )
  ```
  ]
]
