# `@tola/address`

生成 slug，在站点路由、输出路径和浏览器 URL 之间转换。

用 `output-of` 得到传给 `#document` 的输出路径，用 `output-url` 生成链接。

## `slugify`

把标题或其他文本转成可用于 URL 或文件名的 slug。

例如，`slugify("Deep Notes")` 返回 `"deep-notes"`。结果保留字母、组合标记、数字和可用于文件名与 URL 的标点。其他字符用来分隔单词。结果采用 Unicode NFC。

文本里没有可用的字符时会报错。用 `mode: "ascii"` 转写汉字时，`ja` 按日语读，其他语言标签按中文读。

汉字按词读音，多音字的读音由所在词决定。

示例：用默认设置和自定义选项生成 slug：

```typst
#import "@tola/address:0.0.0": slugify
#assert.eq(slugify("Deep Notes"), "deep-notes")
#assert.eq(slugify("Deep Notes", case: "upper", separator: "_"), "DEEP_NOTES")
```

示例：把汉字与带重音的文本转写成 ASCII：

```typst
#import "@tola/address:0.0.0": slugify
#assert.eq(slugify("北京 Café", mode: "ascii"), "bei-jing-cafe")
#assert.eq(slugify("重庆", mode: "ascii"), "chong-qing")
```

### `text`

要变成 slug 的文本。

### `case`

字母大小写。省略时用 `lower`，也可选 `upper`、`capitalize` 或 `preserve`。

### `language`

`ascii` 模式下用来选择汉字读音的 tag，例如 `"zh"`、`"ja"`，默认中文。主 `ja` 子 tag 按日语处理，不区分大小写（`"JA-JP"` 也能匹配）；其他 tag 一律按中文处理。

### `mode`

规范化模式：`unicode`（默认，保留文字本身）或 `ascii`（把汉字按读音、带重音的字母转写成 ASCII）。

### `separator`

单词之间的分隔符。省略时用 `-`，也可传 `_`。

## `join-route`

把 `segments` 里的各段拼成一条路由。

每一段都保留原样。要转写或改变大小写，先对各段调用 `slugify`。

结果以 `/` 开头和结尾，空数组返回 `/`。每一段都要是有效的文件名，不能留空、写成 `.` 或 `..`，或包含斜杠和反斜杠。

示例：用标题生成文章的路由：

```typ
#import "@tola/address:0.0.0": join-route, output-of, slugify
#let route = join-route(("posts", "Deep Notes").map(slugify))
#assert.eq(output-of(route), "posts/deep-notes/index.html")
#document(output-of(route), title: [Deep Notes])[Body]
```

示例：根据源文件路径生成路由：

```typst site
#import "@tola/address:0.0.0": join-route
#import "@tola/source:0.0.0": all-sources
#let routes = all-sources().map(source => join-route(source.route-segments))
#assert.eq(routes, (
  "/",
  "/guide/",
  "/guide/install/",
  "/guide/deploy/",
  "/guide/advanced/plugins/",
  "/notes/first-post/",
  "/notes/second-post/",
))
```

## `output-of`

根据路由得到输出文件路径，可传给 `#document`。

以 `/` 结尾时，返回该目录下的 `index.html`。例如，`/` 返回 `index.html`，`/guide/` 返回 `guide/index.html`。其余路由保留文件名，例如 `/manual.pdf` 返回 `manual.pdf`。`%` 和 `#` 都是文件名字符。

输入不带部署前缀。手上的路径如果已经做过百分号编码，先用 `decode-url-path` 解码一次。

示例：由 route 得到输出文件：

```typst
#import "@tola/address:0.0.0": output-of
#assert.eq(output-of("/"), "index.html")
#assert.eq(output-of("/guide/"), "guide/index.html")
#assert.eq(output-of("/manual.pdf"), "manual.pdf")
```

示例：把字面 `%` 或 `#` 保留为文件名字符：

```typst
#import "@tola/address:0.0.0": output-of
#assert.eq(output-of("/100%/"), "100%/index.html")
#assert.eq(output-of("/a#b.html"), "a#b.html")
```

## `route-of`

根据输出文件路径得到路由。

目录首页用目录路径。例如，`index.html` 返回 `/`，`guide/index.html` 返回 `/guide/`。其他文件保留文件名，例如 `404.html` 返回 `/404.html`。

结果不带部署前缀、origin 或百分号编码。`output-of(route-of(output))` 能把 output 还原；把 `/guide/index.html` 先转成输出再转回来，得到的是规范 route `/guide/`。

示例：由输出文件还原 route：

```typst
#import "@tola/address:0.0.0": route-of
#assert.eq(route-of("index.html"), "/")
#assert.eq(route-of("guide/index.html"), "/guide/")
#assert.eq(route-of("404.html"), "/404.html")
```

## `output-url`

把输出路径转成浏览器 URL，默认加上 `site.base-path`。

传入相对于 `build.publish-dir` 的输出路径，例如 `guide/index.html`。默认加上 `site.base-path`，传 `base-path: "/"` 可省略部署前缀。要生成完整 URL，传 `origin: site.origin` 或指定其他 origin。

文件名恰为 `index.html` 时使用目录 URL：`guide/index.html` 变成 `/guide/`。`asset-url` 则保留声明中的精确文件名，包括 `index.html`。

`base-path` 必须以 `/` 开头和结尾。`origin` 只写 HTTP(S) 协议、主机和可选端口，例如 `https://example.test:8080`。输出名里的 `%` 会编码成 `%25`。

可以传入 hook 稍后才会生成的文件路径。构建时，Tola 会检查写到 HTML 里的链接。

示例：为站内链接应用部署前缀：

在新站点中将这段代码保存为 `site.typ`，再运行 `tola build`。首页会链接到 `/guide/`。如果 `site.base-path` 为 `/docs/`，链接是 `/docs/guide/`。

```typst
#import "@tola/address:0.0.0": output-url
#let guide-output = "guide/index.html"
#document("index.html", title: [Home])[
  #link(output-url(guide-output))[Read the guide]
]
#document(guide-output, title: [Guide])[
  #title()
  Start here.
]
```

示例：从输出路径生成链接：

```typst
#import "@tola/address:0.0.0": output-url
#assert.eq(output-url("guide/index.html"), "/guide/")
#assert.eq(output-url("Hello World/index.html", base-path: "/docs/"), "/docs/Hello%20World/")
#assert.eq(
  output-url("guide/index.html", base-path: "/docs/", origin: "https://example.test"),
  "https://example.test/docs/guide/",
)
```

示例：对输出名中的字面 `%20` 编码一次：

```typst
#import "@tola/address:0.0.0": output-url
#assert.eq(output-url("100%20.html"), "/100%2520.html")
```

示例：链接 hook 生成的文件：

```typst
#import "@tola/address:0.0.0": output-url
#assert.eq(output-url("assets/pagefind-search/pagefind.js"), "/assets/pagefind-search/pagefind.js")
```

### `output`

`build.publish-dir` 下解码后的文件路径，不带前导 `/` 或部署前缀。

### `base-path`

站点的部署路径，例如 `/docs/`。省略或传 `auto` 时，用 `site.base-path`。

### `origin`

传入 `https://example.test` 这样的 origin 可得到完整 URL。省略或传 `none` 时，返回 `/docs/guide/` 这样的路径。

## `route-url`

根据路由生成浏览器 URL，默认加上 `site.base-path`。

相当于先用 `output-of` 取输出路径，再用 `output-url` 生成链接。例如，`/guide/` 和 `/guide/index.html` 都生成指向目录首页的链接。

传入不带部署前缀的路由。如果路径已经做过百分号编码，先用 `decode-url-path` 解码一次。`base-path` 和 `origin` 的用法同 `output-url`。

示例：给路由加部署前缀并生成链接：

```typst
#import "@tola/address:0.0.0": route-url, decode-url-path
#assert.eq(route-url("/guide/index.html", base-path: "/"), "/guide/")
#assert.eq(route-url("/manual.pdf", base-path: "/"), "/manual.pdf")
#assert.eq(route-url("/中文/100%/", base-path: "/docs/"), "/docs/%E4%B8%AD%E6%96%87/100%25/")
#assert.eq(
  route-url(decode-url-path("/caf%C3%A9/"), base-path: "/docs/", origin: "https://example.test"),
  "https://example.test/docs/caf%C3%A9/",
)
```

### `route`

要转换的解码后站点根 route。

### `base-path`

站点的部署路径，例如 `/docs/`。省略或传 `auto` 时，用 `site.base-path`。

### `origin`

传入 `https://example.test` 这样的 origin 可得到完整 URL。省略或传 `none` 时，返回 `/docs/guide/` 这样的路径。

## `asset-url`

生成 `[assets]` 中静态资源的链接。

传入 `[assets]` 里声明的 URL 路径，省略部署前缀。

- `assets.files`：条目自己的 `url`，例如 `"/styles/base.css"`。
- `assets.trees`：树的 `url-prefix` 加上成员在树内的路径；`url-prefix = "/assets"`、树里有 `images/logo.svg` 时，对应 `asset-url("/assets/images/logo.svg")`。

设为 `follow-file-symlinks = true` 后，Tola 用符号链接的名字发布它指向的文件。例如，`static/downloads/guide.txt` 指向 `shared/notes.txt`，这条树的 `source` 是 `"static/downloads"`，`url-prefix` 是 `"/downloads"`。使用默认 `site.base-path` 时，浏览器访问 `/downloads/guide.txt`，读到的是 `notes.txt` 的内容。在 Typst 里写 `asset-url("/downloads/guide.txt")`。完整配置见 `tola help --lang zh "[assets]"`。

路径里的 Unicode 字符和空格可以直接写，也可以编码。文件名里的 `%` 和 `#` 要写成 `%25` 和 `%23`。参数只接受路径。`index.html` 也会保留原文件名。开启 `assets.cache-busting` 后，链接会带上 `?h=<hash>`。

返回的链接已带 `site.base-path`。引用 `[assets]` 中不存在的资源时会报错。要引用 `resize-image` 生成的图片，直接用它返回的 `url`。

两者的区别：

| | `output-url(output)` | `asset-url(声明的 URL)` |
| --- | --- | --- |
| 输入 | 解码后的 output：`build.publish-dir` 下的文件路径，不带前导 `/`（`assets/logo.png`） | `[assets]` 声明拼写的站点根 URL，带前导 `/`（`/assets/logo.png`）；遵循 URL 语法，字面 `%` 和 `#` 必须转义 |
| 检查文件 | 生成链接时不检查，可传入稍后由 hook 写出的文件 | 资源必须在 `[assets]` 中启用 |
| 结果 | 带部署前缀，目录首页用 `/assets/` | 带部署前缀，保留 `/assets/index.html`，开启 `assets.cache-busting` 时加 `?h=<hash>` |

编辑器检查返回的 URL 省略 `?h=…`。用 `tola check` 可以检查构建时实际生成的 URL。

链接 hook 生成的文件时，把输出路径直接传给 `output-url`，例如 `output-url("assets/search.js")`。

示例：链接 `/assets` 下声明的资源：

```typst site
#import "@tola/address:0.0.0": asset-url
#document("index.html")[
  #html.img(src: asset-url("/assets/logo.png"), alt: "Example logo")
]
```

### `declared-url`

`[assets]` 声明的资源路径，例如 `"/app.js"`。

## `decode-url-path`

把 URL 路径里的百分号转义解码一次，得到可传给 `output-of` 的路由。

例如，`/caf%C3%A9/` 变成 `/café/`，`/Hello%20World/` 变成 `/Hello World/`。

只传路径，不带域名、查询参数或锚点。编码后的斜杠或反斜杠也会报错。

解码后的每一段都要能作为跨平台文件名。文件是否已经发布不影响这一步。

要校验文章元数据中的固定链接，用 `@tola/schema` 的 `url-path`。它会指出哪个字段出错，成功时可把结果直接传给 `output-of` 或 `route-url`。

示例：把百分号编码的路径恰好解码一次：

```typst
#import "@tola/address:0.0.0": decode-url-path
#assert.eq(decode-url-path("/caf%C3%A9/"), "/café/")
#assert.eq(decode-url-path("/Hello%20World/"), "/Hello World/")
#assert.eq(decode-url-path("/plain/"), "/plain/")
```

示例：从已编码的路径得到输出文件名：

```typst
#import "@tola/address:0.0.0": decode-url-path, output-of
#assert.eq(decode-url-path("/100%25/"), "/100%/")
#assert.eq(output-of(decode-url-path("/100%25/")), "100%/index.html")
```
