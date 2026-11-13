# `@tola/head`

HTML head 条目与社交卡片

## `head-metadata`

返回字符编码、viewport、标题和描述，供 `html.head` 使用。把下面的代码保存为新站点的 `site.typ`，再运行 `tola build`。生成的 `public/guide/index.html` 会在 HTML head 中包含标题和 canonical 链接。

这些条目是字符集、viewport、标题和描述。head 由你的模板写入：把这些条目拼进 `html.head(...)`，文档自己的 metadata 单独处理。`+` 拼接数组，条目保持扁平且按顺序排列：

```typst
#import "@tola/site:0.0.0": site
#import "@tola/head:0.0.0": canonical, head-metadata
#let entries = head-metadata(site, title: "Guide") + canonical("https://example.test/guide/")
#document("guide/index.html")[
  #html.html[#html.head(entries.join()) #html.body[Guide]]
]
```

Example - 省略标题与描述条目：

```typst
#import "@tola/site:0.0.0": site
#import "@tola/head:0.0.0": head-metadata
#let entries = head-metadata(site, title: none, description: none)
#assert.eq(entries.len(), 2)
```

### `site`

默认值的来源，通常就是 `@tola/site` 提供的字典。

### `charset`

文档的字符编码；除非你传入 `none`，否则写入该条目。

### `description`

`auto` 继承站点描述；`none` 或空字符串则不写入该条目。

### `title`

`auto` 继承站点标题；`none` 或空字符串则不写入该条目。

### `viewport`

viewport meta 的内容；除非你传入 `none`，否则写入该条目。

## `canonical`

返回包含 canonical 链接的数组，供 `html.head` 使用。

一个页面只声明一个 canonical URL，这个条目就是 head 里的 `<link rel="canonical">`。

Example - 声明页面的 canonical URL：

```typst
#import "@tola/head:0.0.0": canonical
#let entries = canonical("https://example.test/guide/")
#assert.eq(entries.len(), 1)
```

### `href`

页面的绝对 URL，协议必须是 `http` 或 `https`。

## `open-graph`

页面的 Open Graph 条目，作为数组拼进 head。

`title` 和 `url` 必填。`kind` 默认 `website`；`images` 可为 `none`，此时输出纯文本卡片，但不含协议要求的 `og:image`，不是完整的 Open Graph 对象。

可选文本非空时才会写入条目。

Example - 声明页面的 Open Graph 条目：

```typst
#import "@tola/head:0.0.0": open-graph
#let entries = open-graph(
  title: "Guide", kind: "website", url: "https://example.test/guide/",
  images: ((url: "https://example.test/cover.png", alt: "Cover"),),
)
#assert.eq(entries.len(), 5)
```

Example - 声明纯文本卡片：

```typst
#import "@tola/head:0.0.0": open-graph
#let entries = open-graph(
  title: "Guide", url: "https://example.test/guide/",
)
#assert.eq(entries.len(), 3)
```

### `title`

页面的标题。

### `kind`

页面的 Open Graph 类型，例如 `website` 或 `article`；默认为 `website`。

### `url`

页面的绝对 URL。

### `images`

`none` 或非空的图片字典数组。`none` 不写入 `og:image`，输出纯文本卡片。每项必填绝对 HTTP(S) `url` 与非空白的 `alt`。可选字段为 `secure-url`（绝对 HTTPS URL）、`media-type`（例如 `image/png` 的字符串）、`width` 与 `height`（正的整数像素数）。可选字段接受 `none`；空白文本不写入条目。每张图片的条目保持在一起，图片按你给出的顺序排列。

### `description`

页面的描述；设置后才写入。

### `site-name`

站点名称；设置后才写入。

### `locale`

页面的 locale；设置后才写入。

## `twitter-card`

页面的 Twitter 卡片条目，作为数组拼进 head。

`card` 和 `title` 总是写入，`summary_large_image` 卡片还需要 `image`。可选文本非空时才会写入条目。

Example - 声明大图卡片：

```typst
#import "@tola/head:0.0.0": twitter-card
#let entries = twitter-card(
  card: "summary_large_image", title: "Guide",
  image: (url: "https://example.test/cover.png", alt: "Cover"),
)
#assert.eq(entries.len(), 4)
```

### `card`

卡片类型：`"summary"` 或 `"summary_large_image"`。

### `title`

页面的标题。

### `image`

大图卡片必填：一个 `(url:, alt:)` 字典。

### `description`

页面的描述；设置后才写入。

### `handle`

站点的 handle；设置后才写入。

### `creator`

创作者的 handle；设置后才写入。
