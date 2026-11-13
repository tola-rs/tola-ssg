# `@tola/sitemap`

为指定页面生成 sitemap

## `sitemap`

为指定页面生成 sitemap。

在 `tola.toml` 的 `[site]` 下设置 `origin = "https://example.com"`。把下面的代码加到 Typst 入口文件，默认是 `site.typ`。如果这两个页面已经存在，只加 import 和 `#sitemap`。

```typst
#import "@tola/sitemap:0.0.0": sitemap
#document("index.html")[Home]
#document("guide/index.html")[Guide]
#sitemap(targets: (
  "index.html",
  (target: "guide/index.html", lastmod: datetime(year: 2026, month: 9, day: 1)),
))
```

从站点根目录运行 `tola build`。按默认设置，`public/sitemap.xml` 会列出 `https://example.com/` 和 `https://example.com/guide/`，并为指南写入最后修改日期。

每个 `target` 必须指向不同的完整文档。指向文档内元素或带 `fragment` 的目标会报错。

`lastmod` 接受 Typst 日期、`YYYY-MM-DD` 字符串或 `@tola/schema` 支持的时间戳。日期只保留年月日。Typst `datetime` 按 UTC 解释，时间戳字符串保留时区偏移和最多九位小数。已有解析结果时，传入它的 `.rfc3339`。

### `output`

输出路径，相对于发布目录。默认是 `sitemap.xml`。

### `targets`

sitemap 列出的文档，每一项是输出路径、label、location，或为其注明日期的 `(target:, lastmod:)` 字典。
