# `@tola/head`

给 HTML 的 `<head>` 添加标题、描述、规范链接和分享卡片。

## `head-metadata`

返回字符编码、viewport、标题和描述，供 `html.head` 使用。

用 `+` 拼接条目数组，再把 `entries.join()` 传给 `html.head`。`#document` 的标题仍可作为文档 metadata 读取。

示例：为页面设置标题、描述和分享卡片：

用这段代码替换新站点的 `site.typ`。换成你自己的页面和封面图的绝对 URL。要随站点一起发布封面图，用 `[assets]`。

```typst
#import "@tola/site:0.0.0": site
#import "@tola/head:0.0.0": canonical, head-metadata, open-graph, twitter-card

#let url = "https://example.test/guide/"
#let cover = (url: "https://example.test/cover.png", alt: "A map of the trail")
#let entries = (
  head-metadata(site, title: "Trail guide", description: "Plan a day hike.")
  + canonical(url)
  + open-graph(title: "Trail guide", url: url, images: (cover,))
  + twitter-card(card: "summary_large_image", title: "Trail guide", image: cover)
)

#document("guide/index.html", title: "Trail guide")[
  #html.html[
    #html.head(entries.join())
    #html.body[Trail guide]
  ]
]
```

运行 `tola build --minify=false`，打开 `public/guide/index.html` 查看 `<head>`：

```html
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Trail guide</title>
  <meta name="description" content="Plan a day hike.">
  <link rel="canonical" href="https://example.test/guide/">
  <meta property="og:title" content="Trail guide">
  <meta property="og:type" content="website">
  <meta property="og:url" content="https://example.test/guide/">
  <meta property="og:image" content="https://example.test/cover.png">
  <meta property="og:image:alt" content="A map of the trail">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="Trail guide">
  <meta name="twitter:image" content="https://example.test/cover.png">
  <meta name="twitter:image:alt" content="A map of the trail">
</head>
```

示例：省略标题与描述条目：

```typst
#import "@tola/site:0.0.0": site
#import "@tola/head:0.0.0": head-metadata
#let entries = head-metadata(site, title: none, description: none)
#assert.eq(entries.len(), 2)
```

### `site`

站点设置，用来读取默认的标题和描述。通常传入 `@tola/site` 的 `site`。

### `charset`

字符编码，默认是 `utf-8`。传 `none` 或空白字符串可省略此标签。

### `description`

省略或传 `auto` 时用站点描述。传 `none` 或空白字符串时省略该标签。

### `title`

省略或传 `auto` 时用站点标题。传 `none` 或空白字符串时省略该标签。

### `viewport`

viewport 的值，默认是 `width=device-width, initial-scale=1`。传 `none` 或空白字符串可省略此标签。

## `canonical`

返回包含 canonical 链接的数组，供 `html.head` 使用。

一个页面只声明一个 canonical URL，这个条目就是 head 里的 `<link rel="canonical">`。


```typst
#import "@tola/head:0.0.0": canonical
#let entries = canonical("https://example.test/guide/")
#assert.eq(entries.len(), 1)
```

### `href`

页面的绝对 URL，协议必须是 `http` 或 `https`。

## `open-graph`

生成分享到支持 Open Graph 的平台时使用的卡片标签。

必须填写 `title` 和 `url`。`kind` 默认是 `website`。直接传 `images: none` 会省略 `og:image`。要生成符合 Open Graph 要求的卡片，至少填一张图。

可选文本非空时才会写入条目。

示例：声明带图片的 Open Graph 条目：

```typst
#import "@tola/head:0.0.0": open-graph
#let entries = open-graph(
  title: "Guide", kind: "website", url: "https://example.test/guide/",
  images: ((url: "https://example.test/cover.png", alt: "Cover"),),
)
#assert.eq(entries.len(), 5)
```

示例：省略图片标签：

```typst
#import "@tola/head:0.0.0": open-graph
#let entries = open-graph(
  title: "Guide", url: "https://example.test/guide/",
)
#assert.eq(entries.len(), 3)
```

### `title`

页面标题，不能只含空白。

### `kind`

Open Graph 类型，例如 `website` 或 `article`。默认是 `website`，不能只含空白。

### `url`

页面的绝对 HTTP(S) URL。

### `images`

传入非空的图片字典数组。每张图片的标签列在一起，按数组顺序输出。传 `none` 时省略图片标签。完整的 Open Graph 对象需要图片。

- `url`：必填，绝对 HTTP(S) URL。
- `alt`：必填，替代文本，不能只含空白。
- `secure-url`：可选，绝对 HTTPS URL。
- `media-type`：可选，例如 `image/png`。
- `width`、`height`：可选，单位为像素，须是正整数。

可选字段接受 `none`。空白文本也会省略。

### `description`

页面描述。省略、传 `none` 或空白字符串时不写入。

### `site-name`

站点名称；设置后才写入。

### `locale`

页面的地区语言标记，例如 `en_US`。可省略。

## `twitter-card`

生成 Twitter 分享卡片的标签。

`card` 和 `title` 总是写入，`summary_large_image` 卡片还需要 `image`。可选文本非空时才会写入条目。


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

页面标题，不能只含空白。

### `image`

`summary_large_image` 必填。传入 `(url:, alt:)` 字典，填写绝对 HTTP(S) URL 和非空白的替代文本。

### `description`

页面描述。省略、传 `none` 或空白字符串时不写入。

### `handle`

站点的 Twitter 账号，可省略。

### `creator`

作者的 Twitter 账号，可省略。
