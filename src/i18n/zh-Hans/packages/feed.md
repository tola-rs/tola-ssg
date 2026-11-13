# `@tola/feed`

为文章生成 RSS、Atom 或 JSON Feed。

## `feed`

在 Typst 入口文件（新站点默认为 `site.typ`）中调用 `feed`，生成 RSS、Atom 或 JSON Feed。

#### 条目与格式

feed 标题取自 `title` 或 `site.title`，不能为空或全是空白。每条内容都要写 `target` 和 `published`。

`target` 可以填输出路径、`(output:, fragment:)` 字典、Typst label 或 location。条目的 `id` 默认用目标 URL，`title` 默认用目标文档的标题。还可以设置 `updated`、`summary`、`content` 和 `authors`。

用 `summary` 写摘要。需要在 feed 里附上正文时，另设 `content`。

| 格式 | 摘要 | 正文 |
| --- | --- | --- |
| RSS | `<description>` | `<content:encoded>`，供读取全文的阅读器使用 |
| Atom | `<summary>` | `<content>`：纯字符串为 `text`，片段或文档选择为 `html` |
| JSON Feed | `summary`，转为纯文本 | 字符串为 `content_text`，片段或文档选择为 `content_html` |

省略正文或传 `none` 时，RSS 和 Atom 省略对应的 XML 元素，JSON Feed 写入空的 `content_text`。

Atom 的 `updated` 默认取 `published`。RSS 省略修改日期。JSON Feed 在条目设置了 `updated` 时写入 `date_modified`。

#### 条目正文

字符串按纯文本写入。Typst `content` 支持文本、换行、强调、粗体、删除线和 URL 链接。Tola 直接把这些元素转成 HTML，不执行 show rule 或排版。比如 `[*Note*]` 会变成 `<strong>Note</strong>`。带样式、依赖 `context` 或已经处理过的 `content` 会报错。

链接可以是相对路径、fragment 或绝对 `http`、`https`、`mailto`、`tel` URL。相对链接以条目的目标 URL 为准。

要包含标题、表格、图片、公式或渲染后的代码，从生成的 HTML 中取正文。`content: (document: output, id: "post-body")` 取该元素及其后代。省略 `id` 或传 `none` 时取整个 `<body>`。`summary` 只接受纯文本或受支持的 Typst `content`。指定的 `id` 必须非空，并在目标 HTML 中恰好出现一次。

提取的 HTML 保留包住正文的元素，以及 `<head>` 中的样式和样式表链接。HTML 文档容器会转成 `div`。URL 属性以原文档和第一个 `<base href>` 为准，转成绝对 URL。CSS 中的 URL 保持原样，样式层叠也可能与原页面不同。

#### ID、锚点和日期

条目 `id` 必须非空，在同一个 feed 中不能重复。文章移动后，已经对外发布的条目 `id` 应继续沿用。Atom 的 feed 和条目 `id` 都必须是绝对 URI。Atom 要有作者，可以统一写在 feed 上，也可以为非空 feed 的每个条目分别指定。提取 HTML 用的 `content.id` 填元素的 ID。

如果 `target` 指向文档内的 label 或 location，用 `#link(<label>)`、`@label` 或 `#outline()` 在站点里链接到它。Typst 导出时才会为该元素生成 `id`。直接指定 `fragment` 时，它也必须指向已导出的锚点。指向整篇文档的 label 或 location 可以直接使用。

`published` 和 `updated` 接受带完整日期的 Typst `datetime`，或带时区的 RFC 3339 字符串。`datetime` 按 UTC 解释，省略时间时取午夜。字符串只写日期、带首尾空白或使用闰秒都会报错。使用 `@tola/schema` 的 `timestamp` 时，传入返回值的 `.rfc3339`。feed 保留数字时区偏移和最多九位小数，RSS 只保留整秒。

示例：添加 RSS feed：

在 `tola.toml` 的 `[site]` 下设置 `origin = "https://example.com"`，用于生成绝对 URL。把下面的代码加到 `site.typ`。

```typst
#import "@tola/feed:0.0.0": feed
#document("notes/index.html", title: [一则 Typst 笔记])[我用 Typst 整理笔记。]
#feed(
  title: "笔记",
  entries: ((
    id: "urn:notes:first",
    target: "notes/index.html",
    published: datetime(year: 2026, month: 9, day: 1),
    summary: "我的第一则 Typst 笔记。",
  ),),
)
```

运行 `tola build`。按默认设置，`public/feed.xml` 会包含这条摘要和链接 `https://example.com/notes/`。

示例：在 feed 中附上文章正文：

用下面这段替换前面的 RSS 示例。`content.id` 选中 `<article>`，把文章正文放进 feed。

```typst
#import "@tola/feed:0.0.0": feed
#let output = "notes/index.html"
#document(output, title: [A note])[
  #html.html[
    #html.head(html.title("A note"))
    #html.body[
      #html.nav[Site navigation]
      #html.article(id: "post-body")[
        #title()
        = A useful idea
        Keep this paragraph in the feed.
      ]
    ]
  ]
]
#feed(
  title: "Notes",
  entries: ((
    id: "urn:notes:first",
    target: output,
    published: datetime(year: 2026, month: 9, day: 1),
    summary: [A useful idea in brief.],
    content: (document: output, id: "post-body"),
  ),),
)
```

示例：单独给订阅者写一份正文：

把这段放在 `content/note.typ` 开头，下面照常写文章正文。在 `site.typ` 生成条目时，从文章的元数据读取 `feed-summary` 和 `feed-content`，分别传给 `summary` 和 `content`。

```typst site
#import "@tola/source:0.0.0": tola-meta
#tola-meta((
  title: "A note",
  published: datetime(year: 2026, month: 9, day: 1),
  feed-summary: [An idea in _brief_.],
  feed-content: [*The full feed text.* Read #link("https://example.test/more/")[more].],
))
```

### `output`

相对于发布目录的文件路径。省略或传 `auto` 时，RSS 用 `feed.xml`，Atom 用 `atom.xml`，JSON Feed 用 `feed.json`。

### `format`

省略时生成 `rss`。也可设为 `atom` 或 `json`。

### `id`

feed 的标识符。省略或传 `auto` 时，用 feed 文件的绝对 URL。

### `title`

feed 的标题。省略或传 `auto` 时，用站点标题。

### `description`

feed 的描述。省略或传 `auto` 时，用站点描述。

### `language`

feed 的语言标签。省略或传 `auto` 时，用站点的语言标签。

### `authors`

feed 的作者。省略或传 `auto` 时，用站点的作者列表。

### `entries`

按发布顺序传入条目。省略时生成空 feed。
