#show: package(
  summary: [读取图片元数据、缩放转换图片，以及生成带 `srcset` 的响应式 `<img>`。],
  related: ("[assets]", "@tola-scripts/image"),
)

生成的图片统一输出到 `_tola/images/` 下，源图与选项相同的多次调用会自动复用同一产物。如需同时发布原图，请使用 #ref("[assets]")。暂不支持动图输入。

#export(
  "resize-image",
  summary: [缩放或转换图片并发布到 `_tola/images/`，返回 URL 与像素尺寸。],
  params: (
    source: [文件路径（相对当前 `.typ` 文件，或 `path(...)` 值）或图片字节。],
    width: [目标宽度（正整数像素）。`fit-width`、`scale`、`fit` 与 `fill` 必填。],
    height: [目标高度（正整数像素）。`fit-height`、`scale`、`fit` 与 `fill` 必填。],
    op: [缩放模式：`"fill"`（默认，裁剪至目标尺寸）、`"fit"`（等比放入且不放大）、`"fit-width"`、`"fit-height"` 或 `"scale"`（拉伸）。],
    anchor: [`"fill"` 裁剪锚点：`"center"`（默认）、`"top"`、`"bottom"`、`"left"`、`"right"`、`"top-left"`、`"top-right"`、`"bottom-left"` 或 `"bottom-right"`。其余模式仅接受 `"center"`。],
    format: [`"auto"`（默认：无透明通道的有损源图转 JPEG，其余转 PNG）、`"jpg"`/`"jpeg"`、`"png"` 或 `"webp"`。SVG 必须显式指定位图格式。],
    quality: [JPEG 取 `1..100`（默认 `75`）；WebP 取 `0..100`（`none` 为无损）。PNG 忽略。],
    filter: [位图重采样滤镜：`"lanczos3"`（默认）、`"nearest"`、`"triangle"`、`"catmull-rom"` 或 `"gaussian"`。SVG 忽略。],
    background: [填充透明区域的不透明 `color`。含透明通道图片转为 JPEG 时必填。],
  ),
  related: ("image-metadata", "image-srcset", "img"),
)[
  返回 `(url:, width:, height:, original-width:, original-height:, format:, mime:)`，其中 `url` 已附带 `site.base-path`。JPEG 宽高上限为 65,535 px（4:2:0 色度采样），WebP 上限为 16,383 px；SVG 内引用图片须用 data URI。

  #example(title: [将封面图按宽度缩放为 WebP], check: "site")[
  ```typst
  #import "@tola/image:0.0.0": resize-image

  #let thumb = resize-image("assets/logo.png", width: 200, op: "fit-width", format: "webp")
  #document("index.html")[
    #html.img(src: thumb.url, width: str(thumb.width), height: str(thumb.height), alt: "标志")
  ]
  ```
  ]
]

#export(
  "image-metadata",
  summary: [读取图片经方向校正后的像素尺寸、格式、MIME 类型及透明/有损标记。],
  params: (
    source: [文件路径或图片字节。],
  ),
  related: ("resize-image",),
)[
  返回 `(width:, height:, format:, mime:, has-alpha:, is-lossy:)`，其中 `format` 为 `"jpg"`、`"png"`、`"webp"`、`"gif"`、`"bmp"` 或 `"svg"`。
]

#export(
  "image-srcset",
  summary: [为位图生成多宽度版本，返回可直接使用的 `srcset` 字符串与回退 URL。],
  params: (
    source: [位图文件路径或字节。],
    widths: [`auto`（默认，取原图宽度）或正整数像素宽度数组（自动排序去重）。],
    format: [输出格式：`"webp"`（默认）、`"auto"`、`"jpg"`/`"jpeg"` 或 `"png"`。],
    quality: [JPEG `1..100` 或 WebP `0..100`（默认 `75`；传 `none` 输出无损 WebP）。PNG 忽略。],
    background: [含透明通道图片转为 JPEG 时必填的不透明底色 `color`。],
  ),
  related: ("img", "resize-image", "@tola/head#open-graph"),
)[
  返回 `(url:, width:, height:, srcset:, variants:)`，`url` 指向最宽的生成版本（已带 `site.base-path`）。
]

#export(
  "img",
  summary: [生成带像素宽高与自动 `srcset`/`sizes` 的响应式 HTML `<img>` 元素。],
  params: (
    source: [位图路径或字节。SVG 请直接用 Typst 的 `image(...)` 或先用 `resize-image` 转换。],
    widths: [`auto`（默认，取原图宽度）或正整数像素宽度数组。],
    alt: [替代文本（默认 `""`，标记装饰图片），始终写入 `<img>`。],
    sizes: [CSS `sizes` 提示（默认 `"100vw"`），仅在去重后至少生成两个宽度时写出。],
    format: [输出格式：`"webp"`（默认）、`"auto"`、`"jpg"`/`"jpeg"` 或 `"png"`。],
    quality: [JPEG `1..100` 或 WebP `0..100`（默认 `75`；`none` 输出无损 WebP）。PNG 忽略。],
    background: [含透明通道图片转为 JPEG 时必填的不透明底色 `color`。],
    attrs: [附加字符串 HTML 属性。默认写入 `loading: "lazy"` 与 `decoding: "async"`，可在此覆盖；由函数托管的属性（`src`、`srcset`、`sizes`、`width`、`height`、`alt`）禁止传入。],
  ),
  related: ("image-srcset", "resize-image"),
)[
  用于 HTML 导出；PDF 等分页格式请使用 Typst 内置的 `image(...)`。

  #example(title: [渲染优先加载的首屏响应式图片], check: "site")[
  ```typst
  #import "@tola/image:0.0.0": img

  #document("index.html")[
    #img(
      "assets/logo.png",
      widths: (480, 960),
      alt: "首屏横幅",
      sizes: "(max-width: 760px) 100vw, 720px",
      attrs: (loading: "eager", fetchpriority: "high"),
    )
  ]
  ```
  ]
]
