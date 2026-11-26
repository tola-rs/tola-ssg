#show: package(
  summary: [生成 HTML `<head>` 元数据标签：标题、描述、canonical 链接、Open Graph 与 Twitter 卡片。],
  related: ("@tola/site", "@tola/image#image-srcset"),
)

每个函数均返回 HTML 元素数组，可用 `+` 拼接后通过 `.join()` 传给 `html.head(...)`。

#export(
  "head-metadata",
  summary: [生成基础的 `<meta charset>`、viewport、`<title>` 与 `<meta name="description">` 标签。],
  params: (
    site: [提供默认标题与描述的站点字典（通常传入 `@tola/site` 的 `site`）。],
    title: [页面标题。`auto` 取 `site.title`；`none` 或空白字符串省略 `<title>`。],
    description: [页面描述。`auto` 取 `site.description`；`none` 或空白字符串省略描述标签。],
    charset: [字符编码，默认 `"utf-8"`；传 `none` 或空白字符串省略。],
    viewport: [viewport 内容，默认 `"width=device-width, initial-scale=1"`；传 `none` 或空白字符串省略。],
  ),
  related: ("canonical", "open-graph", "twitter-card"),
)[
  #example(title: [组合完整的 HTML `<head>` 元数据], check: "default")[
  ```typst
  #import "@tola/head:0.0.0": canonical, head-metadata, open-graph, twitter-card
  #import "@tola/site:0.0.0": site

  #let url = "https://example.com/guide/"
  #let cover = (url: "https://example.com/cover.png", alt: "指南封面")
  #let tags = (
    head-metadata(site, title: "指南", description: "快速上手站点配置。")
    + canonical(url)
    + open-graph(title: "指南", url: url, images: (cover,))
    + twitter-card(card: "summary_large_image", title: "指南", image: cover)
  )

  #document("guide/index.html", title: "指南")[
    #html.html[
      #html.head(tags.join())
      #html.body[指南正文]
    ]
  ]
  ```
  ]
]

#export(
  "canonical",
  summary: [生成页面的 `<link rel="canonical">` 标签。],
  params: (
    href: [页面的绝对 `http://` 或 `https://` URL。],
  ),
  related: ("head-metadata", "open-graph"),
)

#export(
  "open-graph",
  summary: [生成社交分享预览所需的 Open Graph `<meta property="og:*">` 标签。],
  params: (
    title: [非空页面标题。],
    kind: [Open Graph 类型，例如 `"website"`（默认）或 `"article"`。],
    url: [页面的绝对 HTTP(S) URL。],
    images: [传 `none` 省略图片标签，或传入非空的 `(url:, alt:)` 图片字典数组。],
    description: [可选摘要，非空时写入。],
    site-name: [可选站点名，非空时写入。],
    locale: [可选区域标签，非空时写入。],
  ),
  related: ("twitter-card", "head-metadata"),
)

#export(
  "twitter-card",
  summary: [生成 Twitter 分享卡片所需的 `<meta name="twitter:*">` 标签。],
  params: (
    card: [卡片类型：`"summary"` 或 `"summary_large_image"`。],
    title: [非空卡片标题。],
    image: [可选 `(url:, alt:)` 字典；当 `card` 为 `"summary_large_image"` 时必填。],
    description: [可选描述，非空时写入。],
    handle: [可选站点账号（`twitter:site`）。],
    creator: [可选作者账号（`twitter:creator`）。],
  ),
  related: ("open-graph", "head-metadata"),
)
