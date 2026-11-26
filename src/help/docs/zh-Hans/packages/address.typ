#show: package(
  summary: [生成 URL slug，并在站点路由、发布输出路径与浏览器 URL 之间转换。],
  related: ("[site]", "[assets]", "@tola/source"),
)

保持三类地址概念清晰分离：*路由*（`/guide/`）是不带部署前缀的逻辑路径；*输出路径*（`guide/index.html`）是传给 `#document(...)` 的相对 `build.publish-dir` 文件路径；*浏览器 URL*（`/docs/guide/`）附加了 `site.base-path` 与百分号编码。

#export(
  "slugify",
  summary: [将文本规范化为可用于 URL 和文件名的 Unicode NFC slug。],
  params: (
    text: [待转换的源文本；若不含任何可用字符则报错。],
    mode: [`"unicode"`（默认）保留原有字母与数字；`"ascii"` 将汉字按读音、带重音字母按基础拉丁字母转写为 ASCII。],
    case: [字母大小写：`"lower"`（默认）、`"upper"`、`"capitalize"` 或 `"preserve"`。],
    separator: [单词分隔符：`"-"`（默认）或 `"_"`。],
    language: [`"ascii"` 模式下选择汉字读音的 BCP 47 标签。主子标签为 `"ja"`（不区分大小写）按日语读音，其余标签按中文读音。],
  ),
  related: ("join-route", "@tola/source#all-sources"),
)[
  保留字母、组合标记、数字及安全的 URL/文件名标点，其余字符作为词分隔符。`"ascii"` 模式下汉字按词组确定读音，可正确处理常见多音字。

  #example(title: [生成 Unicode 与 ASCII slug], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": slugify

  #let english = slugify("Deep Notes")
  #let ascii = slugify("北京 Café", mode: "ascii")
  #let upper = slugify("Deep Notes", case: "upper", separator: "_")
  ```
  ]
]

#export(
  "join-route",
  summary: [将路径段数组拼接为以 `/` 开头和结尾的站点根路由。],
  params: (
    segments: [文件名段数组。每段均须非空、不能为 `.` 或 `..`，且不含 `/` 与 `\`。],
  ),
  related: ("slugify", "output-of", "@tola/source#all-sources"),
)[
  各段原样保留；若需转小写或转写，请先对各段调用 `slugify`。空数组返回 `"/"`。

  #example(title: [从标题段生成文档输出路径], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": join-route, output-of, slugify

  #let route = join-route(("posts", "Deep Notes").map(slugify))
  #document(output-of(route), title: [Deep Notes])[文章正文]
  ```
  ]
]

#export(
  "output-of",
  summary: [将解码后的站点根路由映射为可传给 `#document` 的输出文件路径。],
  params: (
    route: [不含部署前缀的解码站点根路由（例如 `"/guide/"` 或 `"/manual.pdf"`）。],
  ),
  related: ("route-of", "output-url", "decode-url-path"),
)[
  以 `/` 结尾的路由映射为该目录下的 `index.html`（`"/"` → `"index.html"`，`"/guide/"` → `"guide/index.html"`）；其余路由保留原文件名（`"/manual.pdf"` → `"manual.pdf"`）。字面 `%` 与 `#` 按普通文件名字符处理；若输入已经百分号编码，请先调用 `decode-url-path`。

  #example(title: [将路由转换为文档输出路径], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": output-of

  #let home = output-of("/")
  #let guide = output-of("/guide/")
  #let pdf = output-of("/manual.pdf")
  ```
  ]
]

#export(
  "route-of",
  summary: [将输出文件路径还原为规范的站点根路由。],
  params: (
    output: [相对于 `build.publish-dir` 的解码文件路径，不带前导 `/`。],
  ),
  related: ("output-of", "route-url"),
)[
  目录首页折叠为目录路由（`"index.html"` → `"/"`，`"guide/index.html"` → `"/guide/"`）；其他文件保留文件名（`"404.html"` → `"/404.html"`）。返回值不含 `base-path`、`origin` 或百分号编码。
]

#export(
  "output-url",
  summary: [将输出文件路径转换为带 `site.base-path` 和百分号编码的浏览器 URL。],
  params: (
    output: [相对于 `build.publish-dir` 的解码文件路径，不带前导 `/` 或部署前缀。],
    base-path: [以 `/` 开头和结尾的部署前缀；省略或传 `auto` 时使用 `site.base-path`。],
    origin: [可选 HTTP(S) 源地址（如 `"https://example.com"`），用于生成完整绝对 URL；省略或传 `none` 返回站内路径。],
  ),
  related: ("route-url", "asset-url", "output-of"),
)[
  目录首页（`guide/index.html`）折叠为目录链接（`/guide/`）。文件名中的字面 `%` 会编码一次为 `%25`。调用时不要求文件已存在，因此可直接用于链接 #ref("[[hooks.generate-outputs]]") 稍后生成的文件；Tola 会在 hook 结束后统一校验 HTML 链接。

  #example(title: [生成自动带部署前缀的站内链接], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": output-url

  #let guide = "guide/index.html"
  #document("index.html", title: [Home])[
    #link(output-url(guide))[阅读指南]
  ]
  #document(guide, title: [Guide])[从这里开始。]
  ```
  ]
]

#export(
  "route-url",
  summary: [将解码后的站点根路由转换为带 `site.base-path` 的浏览器 URL。],
  params: (
    route: [不含部署前缀的解码站点根路由。],
    base-path: [以 `/` 开头和结尾的部署前缀；省略或传 `auto` 时使用 `site.base-path`。],
    origin: [可选 HTTP(S) 源地址，用于生成完整绝对 URL。],
  ),
  related: ("output-url", "output-of", "decode-url-path"),
)[
  等价于 `output-url(output-of(route), ...)`：`"/guide/"` 与 `"/guide/index.html"` 均生成目录链接 `"/guide/"`（附加 `base-path`）。若输入已百分号编码，请先用 `decode-url-path` 解码一次。
]

#export(
  "asset-url",
  summary: [获取 `[assets]` 中已声明静态资源的浏览器 URL。],
  params: (
    declared-url: [`[assets]` 中声明的站点根 URL 路径（带前导 `/`、不含 `site.base-path`）。文件名中的字面 `%` 与 `#` 须写作 `%25` 与 `%23`。],
  ),
  related: ("[assets]", "output-url"),
)[
  - `assets.files`：传入条目自身的 `url`（例如 `"/styles/base.css"`）。
  - `assets.trees`：传入 `url-prefix` 与树内相对路径的拼接（例如 `"/assets/images/logo.svg"`）。

  引用的资源未在 #ref("[assets]") 中启用时会立即报错。保留精确文件名（包括 `index.html`），自动附加 `site.base-path`，开启 `assets.cache-busting` 时追加 `?h=<hash>`（编辑器检查会省略 `?h=...`，请用 `tola check` 校验最终 URL）。符号链接规则见 #ref("[assets]")；链接 hook 产物请用 `output-url`；引用缩放图片请直接使用 #ref("@tola/image#resize-image") 返回的 `.url`。

  #table(
    columns: 3,
    table.header([], [`output-url(output)`], [`asset-url(declared-url)`]),
    [输入], [解码后的发布相对路径（`"assets/logo.png"`）。], [`[assets]` 声明的站点根 URL（`"/assets/logo.png"`）。],
    [校验时机], [构建 hook 结束后统一校验。], [调用时即校验资源已在 `[assets]` 中启用。],
    [返回结果], [带 `base-path`，`index.html` 折叠为 `/`。], [带 `base-path`，保留 `index.html`，开启 cache-busting 时附加 `?h=<hash>`。],
  )

  #example(title: [引用已声明的静态图片], check: "site")[
  ```typst
  #import "@tola/address:0.0.0": asset-url

  #document("index.html")[
    #html.img(src: asset-url("/assets/logo.png"), alt: "站点标志")
  ]
  ```
  ]
]

#export(
  "decode-url-path",
  summary: [对 URL 路径做一次百分号解码，得到可传给 `output-of` 或 `route-url` 的站点根路由。],
  params: (
    url-path: [不含协议、主机、查询串或锚点的站点根 URL 路径。拒绝编码后的 `/` 或 `\` 以及非法文件名段。],
  ),
  related: ("output-of", "route-url", "@tola/schema#url-path"),
)[
  例如 `"/caf%C3%A9/"` 解码为 `"/café/"`，`"/100%25/"` 解码为 `"/100%/"`。若需校验文章元数据中的固定链接并输出字段级诊断，优先使用 #ref("@tola/schema#url-path")。
]
