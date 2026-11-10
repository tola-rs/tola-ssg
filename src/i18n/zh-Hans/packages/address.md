# `@tola/address`

slugs、route、output 路径与浏览器 URL

为文档选择 route，把它转成传给 `#document` 的 output，再把 output 转成链接使用的浏览器 URL。这三个值描述同一份文件发布时的不同方面：

| 值 | 含义 | 示例 |
| --- | --- | --- |
| route | 从站点根开始的解码后逻辑地址；以 `/` 开头 | `/Hello World/` |
| output | 写入 `build.publish-dir` 下的文件路径；不带前导 `/` 或部署前缀 | `Hello World/index.html` |
| 浏览器 URL | 链接使用的地址；经过百分号编码，带部署前缀，还可以带 origin | `/docs/Hello%20World/` |

route 的前导 `/` 表示本站的根。末尾的 `/` 区分目录与文件：`/guide/` 对应 `guide/index.html`，`/guide` 则对应文件 `guide`。`output-of` 完成这一步转换；`route-of` 返回 output 的规范 route，所以 `guide/index.html` 变成 `/guide/`。显式 route `/guide/index.html` 与 `/guide/` 命名同一份 output，也会规范化为 `/guide/`。

源路径命名输入文件，例如 `content/hello.typ`，与这三种地址分开。传给 `#document` 的 output 决定该文档发布在哪里。用 `join-route` 组合解码后的 route 分段；需要 slug 时先调用 `slugify`（`slugify("Hello World")` 得到 `"hello-world"`）。

这里 route 与 output 都保留空格，只有浏览器 URL 对空格编码并加上 `/docs/`：

```typst
#import "@tola/address:0.0.0": output-of, route-of, output-url, route-url
#let route = "/Hello World/"
#let output = output-of(route)
#let url = output-url(output, base-path: "/docs/")
#assert.eq(output, "Hello World/index.html")
#assert.eq(route-of(output), route)
#assert.eq(url, "/docs/Hello%20World/")
#assert.eq(route-url(route, base-path: "/docs/"), url)
```

`[site]` 的 `base-path` 指定发布目录对外提供服务的位置：`base-path = "/docs/"` 时，`Hello World/index.html` 仍直接写到 `build.publish-dir` 下，不会额外添加 `docs/` 目录。`output-url` 与 `route-url` 默认使用该配置；传 `origin: site.origin` 可得到绝对 URL。直接使用返回值，再加 `/docs/` 会重复前缀。手写站点根 URL 时要自己带上前缀，Tola 不会改写链接字符串。

route 与 output 都是解码后的文本，`%` 和 `#` 是字面的文件名字符。浏览器 URL 把它们编码为 `%25` 和 `%23`；文件名中的字面 `%20` 会变成 `%2520`。已编码且不带部署前缀的 URL 路径，先用 `decode-url-path` 恰好解码一次，再作为 route 使用。URL 的 query 与 fragment 是独立后缀：链接到章节时，在完整 URL 后追加 `#intro`；解码后 route 内部的 `#` 则命名文件名中的字符。

用 `asset-url` 解析 `[assets]` 声明的不带部署前缀的 URL，例如 `/assets/logo.png`。输入遵循 URL 路径语法：字面 `%` 和 `#` 要转义为 `%25` 和 `%23`，不能带 query 或 fragment。它会校验声明，保留精确文件名，应用 `base-path`，并在开启 cache-busting 时添加 `?h=<asset hash>`。`output-url` 则接受任意 output 路径，不检查文件是否存在，包括 `generate-outputs` hook 通过 `TOLA_HOOK_OUTPUT_DIR` 写出的文件。文档导出、原生 `asset()` 文件、hook output 都位于 `build.publish-dir` 下，不带部署前缀。

## `slugify`

纯函数：把文本变成一个 slug，一个可以直接放进 URL 或文件名的字符串。

slug 里保留字母、标记、数字，以及 URL 和文件名安全的标点：`slugify("Deep Notes")` 得到 `"deep-notes"`；其余字符都用来分隔单词，结果会做 NFC 规范化。

约束：文本里没有任何能构成 slug 的内容时报错；`mode`、`case`、`separator` 只接受各自列出的取值；`mode: "ascii"` 下读音随 `language`：只有主 `ja` 子 tag 按日语读汉字，其他任何 tag（包括 `en`）都按中文读。

Example - 先用默认设置，再带选项，把标题变成 slug：

```typst
#import "@tola/address:0.0.0": slugify
#assert.eq(slugify("Deep Notes"), "deep-notes")
#assert.eq(slugify("Deep Notes", case: "upper", separator: "_"), "DEEP_NOTES")
```

Example - 把汉字与带重音的文本转写成 ASCII；汉字按词读音，多音字跟随它所在的词：

```typst
#import "@tola/address:0.0.0": slugify
#assert.eq(slugify("北京 Café", mode: "ascii"), "bei-jing-cafe")
#assert.eq(slugify("重庆", mode: "ascii"), "chong-qing")
```

### `text`

要变成 slug 的文本。

### `case`

slug 采用的字母大小写：`lower`、`upper`、`capitalize` 或 `preserve`。

### `language`

`ascii` 模式下用来选择汉字读音的 tag，例如 `"zh"`、`"ja"`，默认中文。主 `ja` 子 tag 按日语处理，不区分大小写（`"JA-JP"` 也能匹配）；其他 tag 一律按中文处理。

### `mode`

规范化模式：`unicode`（默认，保留文字本身）或 `ascii`（把汉字按读音、带重音的字母转写成 ASCII）。

### `separator`

单词之间的分隔符：`-` 或 `_`。

## `join-route`

纯函数：把 `segments`（一个数组）拼接为一条解码后的站点根 route。

每个分段都按原样保留，所以想要 slug、其他大小写或转写时，先对分段调用 `slugify`。内容源的 `route-segments` 只是文件布局给出的默认 route；文档实际以哪条 route 可达，由它自己的 output 决定。

约束：结果总是以 `/` 开头、以 `/` 结尾，不传分段就得到站点根 `/`；分段为空、为 `.`、为 `..`，或者包含 `/`、`\`，或者包含已发布文件名无法承载的字符时报错。

Example - 保留分段原样，再逐个 slug 后发布：

```typ
#import "@tola/address:0.0.0": join-route, output-of, slugify
#let route = join-route(("posts", "Deep Notes").map(slugify))
#assert.eq(output-of(route), "posts/deep-notes/index.html")
#document(output-of(route), title: [Deep Notes])[Body]
```

Example - 取文件布局给出的默认 route：

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

纯函数：把解码后的站点根 route 转成其 output 文件路径。

route 是站点内部的逻辑地址，不带 `base-path` 或 `origin`；`%` 和 `#` 是字面的文件名字符。以 `/` 结尾时，route 指向一个目录，该目录按自己的 `index.html` 发布：`/` 是 `index.html`，`/guide/` 是 `guide/index.html`；不带结尾 `/` 时，route 按字面指向文件：`/guide` 是 `guide`，`/manual.pdf` 是 `manual.pdf`。

约束：route 按原样取用，百分号编码的 URL 路径要先用 `decode-url-path` 解码，而且只解码一次。

Example - 由 route 得到输出文件：

```typst
#import "@tola/address:0.0.0": output-of
#assert.eq(output-of("/"), "index.html")
#assert.eq(output-of("/guide/"), "guide/index.html")
#assert.eq(output-of("/manual.pdf"), "manual.pdf")
```

Example - 把字面 `%` 或 `#` 保留为文件名字符：

```typst
#import "@tola/address:0.0.0": output-of
#assert.eq(output-of("/100%/"), "100%/index.html")
#assert.eq(output-of("/a#b.html"), "a#b.html")
```

## `route-of`

纯函数：把 output 文件路径转成其规范的解码后站点根 route。

文件名恰为 `index.html` 时，取包含它的目录地址：`index.html` 是 `/`，`guide/index.html` 是 `/guide/`。其他文件名原样保留在 `/` 下：`404.html` 是 `/404.html`，`about` 是 `/about`。

结果不带部署前缀、origin 或百分号编码。`output-of(route-of(output))` 能把 output 还原；把 `/guide/index.html` 先转成输出再转回来，得到的是规范 route `/guide/`。

Example - 由输出文件还原 route：

```typst
#import "@tola/address:0.0.0": route-of
#assert.eq(route-of("index.html"), "/")
#assert.eq(route-of("guide/index.html"), "/guide/")
#assert.eq(route-of("404.html"), "/404.html")
```

## `output-url`

依赖站点配置（`[site]` 的 `base-path`）：输出文件对外提供服务的浏览器 URL。

传入 `build.publish-dir` 下解码后的 output 路径，不带前导 `/` 或部署前缀，例如 `guide/index.html`：它可以是文档自己的 output，也可以是 `output-of` 的结果。返回的 URL 前面会加上 `[site]` 的 `base-path`（默认 `/`）；传 `base-path: "/"` 覆盖为根部署，传 `origin` 得到绝对 URL（`origin: site.origin` 用站点自己的 origin）。

文件名恰为 `index.html` 时使用目录 URL：`guide/index.html` 变成 `/guide/`。`asset-url` 则保留声明中的精确文件名，包括 `index.html`。

约束：`base-path` 必须以 `/` 开头、以 `/` 结尾，其中的转义只解码一次；`origin` 是一个绝对的 `http`/`https` origin，其中不能有部署路径、凭据、query 或 fragment；输出名只编码一次，所以字面 `%20` 会变成 `%2520`。

任何输出路径都能在这里解析，包括 hook 产出的：这里不检查是否已发布，因为地址只是一个名字，不承诺文件存在。`[assets]` 声明拼写的 URL 不是输出路径——那类由 `asset-url` 解析，并带上它们的字节身份。

文档 output 不加部署前缀；返回的 URL 直接用于链接。最终 reference 检查会校验链接指向的 output 是否存在。

Example - 发布一个页面，并从另一篇文档链接到它：

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

Example - 把输出挂到一个部署路径和一个 origin 下：

```typst
#import "@tola/address:0.0.0": output-url
#assert.eq(output-url("guide/index.html"), "/guide/")
#assert.eq(output-url("Hello World/index.html", base-path: "/docs/"), "/docs/Hello%20World/")
#assert.eq(
  output-url("guide/index.html", base-path: "/docs/", origin: "https://example.test"),
  "https://example.test/docs/guide/",
)
```

Example - 输出名里的字面 `%20` 只编码一次：

```typst
#import "@tola/address:0.0.0": output-url
#assert.eq(output-url("100%20.html"), "/100%2520.html")
```

Example - 链接 hook 自己的产出，它没有任何 `assets` 声明为它命名：

```typst
#import "@tola/address:0.0.0": output-url
#assert.eq(output-url("assets/pagefind-search/pagefind.js"), "/assets/pagefind-search/pagefind.js")
```

### `output`

`build.publish-dir` 下解码后的文件路径，不带前导 `/` 或部署前缀。

### `base-path`

部署路径；`auto` 取站点自身的 `base-path`。

### `origin`

结果的绝对 origin。

## `route-url`

解码后的站点根 route 对应的浏览器 URL，使用其 output 的规范 URL。

它等于 `output-url(output-of(route), base-path: base-path, origin: origin)`：末尾 `/` 命名 index 输出，显式的 `/guide/index.html` 也会变成 `/guide/`。文件 route（如 `/manual.pdf`）保留文件名。它不保留每个 route 的原始拼写，而是选择与 `output-url` 相同的规范地址。

传入不带部署前缀的解码后文本：Unicode、字面 `%` 和 `#` 都是文件名字符。已编码 URL 路径应先通过 `decode-url-path` 恰好解码一次。部署前缀只添加一次；`base-path: auto` 使用 `site.base-path`，`origin` 的约束与 `output-url` 一致。

Example - 规范化 route，再挂载并编码：

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

部署路径；`auto` 取站点自身的 `base-path`。

### `origin`

结果的绝对 origin。

## `asset-url`

依赖站点配置（`[assets]` 声明、`[site]` 的 `base-path`）：已声明资产 URL 对外提供服务的浏览器 URL。

传入声明拼写的不带部署前缀的站点根 URL，不包含 `site.base-path`：

- `assets.files`：条目自己的 `url`，例如 `"/styles/base.css"`。
- `assets.trees`：树的 `url-prefix` 加上成员在树内的路径；`url-prefix = "/assets"`、树里有 `images/logo.svg` 时，对应 `asset-url("/assets/images/logo.svg")`。

参数遵循 URL 路径语法，只进行一次百分号解码。Unicode 和内部空格可以直接写，也可以编码；文件名中的字面 `%` 或 `#` 必须写为 `%25` 或 `%23`。不接受 query 或 fragment，也不要传入另一次调用的解码结果。构建时，它保留声明的精确文件名（包括 `index.html`），并在 `assets.cache-busting` 打开时追加 `?h=<identity>`：内容变了，地址跟着变，名称保持不变。

约束：返回的 URL 以 `/` 开头，并在前面加上 `site.base-path`，直接拿去用；再挂载一次就会重复；没有任何 `assets` 声明发布这个 URL 时调用处报错，即使该路径由别的生产者发布，比如 `resize-image` 的派生结果。

两者的区别：

| | `output-url(output)` | `asset-url(声明的 URL)` |
| --- | --- | --- |
| 输入 | 解码后的 output：`build.publish-dir` 下的文件路径，不带前导 `/`（`assets/logo.png`） | `[assets]` 声明拼写的站点根 URL，带前导 `/`（`/assets/logo.png`）；遵循 URL 语法，字面 `%` 和 `#` 必须转义 |
| 发布检查 | 不检查——hook 将来产出的文件也能解析 | `[assets]` 声明必须发布这个 URL，即使该路径由别的生产者写出 |
| 结果 | 挂载后的规范 URL；`assets/index.html` 使用 `/assets/` | 挂载后的精确文件 URL；`/assets/index.html` 保持 `/assets/index.html`，`assets.cache-busting` 打开时追加 `?h=<identity>` |

编辑器源检查只解析挂载后的地址，不带 identity，因为它不渲染资产字节。`tola check` 会准备完整 output，包括资产 identity，但不发布它们。

hook 自己的产出正是这样的路径：页面用 `output-url("assets/…")` 链接它，两个调用彼此独立——这里传声明的拼写，绝不传另一个函数的结果。

Example - 解析站点在 `/assets` 下发布的文件：

```typst site
#import "@tola/address:0.0.0": asset-url
#document("index.html")[
  #html.img(src: asset-url("/assets/logo.png"), alt: "Example logo")
]
```

### `declared-url`

声明拼写的站点根资产 URL，例如 "/app.js"。

## `decode-url-path`

纯函数：把百分号编码的站点根 URL 路径恰好解码一次。

每个转义都会还原成它所指的字节，所以 `/caf%C3%A9/` 是 `/café/`，`/Hello%20World/` 是 `/Hello World/`。只在这里解码一次。

约束：参数只能是路径——传入 origin、query string 或 fragment 都会报错；转义的 `/` 或 `\` 同样会报错，因为层级只由未转义的 `/` 划分。

解码后的路径还必须满足可移植输出文件名规则；它不是通用 URL 解码器，也不检查路径是否已发布。

metadata 可以使用 `@tola/schema` 的 `url-path`，在应用相同规则的同时收集字段错误。解析结果已经解码，直接交给 `output-of` 或 `route-url` 即可。

Example - 把百分号编码的路径恰好解码一次：

```typst
#import "@tola/address:0.0.0": decode-url-path
#assert.eq(decode-url-path("/caf%C3%A9/"), "/café/")
#assert.eq(decode-url-path("/Hello%20World/"), "/Hello World/")
#assert.eq(decode-url-path("/plain/"), "/plain/")
```

Example - 把路径解码一次，再命名输出文件：

```typst
#import "@tola/address:0.0.0": decode-url-path, output-of
#assert.eq(decode-url-path("/100%25/"), "/100%/")
#assert.eq(output-of(decode-url-path("/100%25/")), "100%/index.html")
```
