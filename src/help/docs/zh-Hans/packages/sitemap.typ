#show: package(
  summary: [为搜索引擎生成包含已发布页面的 XML 站点地图。],
  related: ("[site]", "@tola/feed", "@tola/schema#timestamp"),
)

#export(
  "sitemap",
  summary: [输出包含绝对 URL 与可选 `lastmod` 时间戳的 `sitemap.xml`。],
  params: (
    output: [相对于发布目录的输出路径，默认 `"sitemap.xml"`。],
    targets: [互不重复的完整文档数组：每项可为输出路径、文档 label、location，或 `(target:, lastmod:)` 字典。拒绝文档内元素或带 fragment 的目标。],
  ),
)[
  需要在 #ref("[site]") 中配置 `origin`。`lastmod` 接受 Typst `datetime`（按 UTC 解释）、`"YYYY-MM-DD"` 日期字符串或 RFC 3339 时间戳字符串（使用 #ref("@tola/schema#timestamp") 时传入 `.rfc3339`）。

  #example(title: [生成带最后修改日期的 `sitemap.xml`], check: "site")[
  ```typst
  #import "@tola/sitemap:0.0.0": sitemap

  #document("index.html")[首页]
  #document("guide/index.html")[指南]

  #sitemap(targets: (
    "index.html",
    (target: "guide/index.html", lastmod: datetime(year: 2026, month: 9, day: 1)),
  ))
  ```
  ]
]
