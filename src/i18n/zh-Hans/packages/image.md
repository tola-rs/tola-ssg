# `@tola/image`

读取图片信息、缩放图片，或生成带 `srcset` 的 `<img>`。

## `resize-image`

缩放图片并发布到 `_tola/images/`。把返回的 `url`、`width` 和 `height` 传给 `html.img`。URL 已带部署前缀。`original-width` 和 `original-height` 是源图尺寸。尺寸单位都是像素。

`scale` 分别设置宽高，可能改变宽高比。`fit-width` 和 `fit-height` 按宽高比推算另一边，四舍五入到整数，最少 1 像素。`fit` 将整张图缩进目标框内，源图本来就放得下时保持原尺寸。`fill` 裁剪到目标宽高。除 `fit` 外，其他操作均可放大源图。

#### 图片限制

JPEG 的宽高各不能超过 65,535 像素，使用 4:2:0 色度采样。WebP 的宽高各不能超过 16,383 像素。彩色文字和细线用 PNG 可以保留更多细节。SVG 内嵌的图片须用 data URI，文字使用编译器默认字体。需要固定字形时，先把文字转成路径。动画图片会报错。

用 `[assets]` 同时发布原图。运行 `tola check --json` 可以查看生成的图片路径和媒体类型。

示例：发布 640 像素宽的预览图：

将一张 PNG 放在 `assets/logo.png`，把以下代码存为 `site.typ`，然后运行 `tola build`。Tola 会将 640 像素宽的图片写入 `public/_tola/images/`，并将它放在首页 `public/index.html`。高度按源图宽高比计算。

```typst site
#import "@tola/image:0.0.0": resize-image
#let preview = resize-image("assets/logo.png", width: 640, op: "fit-width")
#document("index.html")[
  #html.img(src: preview.url, width: preview.width, height: preview.height, alt: "Site logo")
]
```

示例：查看原图和缩放后的尺寸：

下面的断言使用一张 8 × 4 像素的 PNG，路径为相对于入口文件的 `assets/logo.png`。换成自己的图片时，`original-*` 的断言要按原图尺寸修改。

```typst site
#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("assets/logo.png", width: 4, height: 4)
#assert.eq(resized.width, 4)
#assert.eq(resized.height, 4)
#assert.eq(resized.original-width, 8)
#assert.eq(resized.original-height, 4)
```

示例：指定宽度，让高度按比例缩放：

```typst site
#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("assets/logo.png", width: 4, op: "fit-width")
#assert.eq(resized.width, 4)
#assert.eq(resized.height, 2)
```

示例：缩放成固定布局的尺寸：

```typst site
#import "@tola/image:0.0.0": resize-image
#let slot = resize-image(
  "assets/logo.png",
  width: 4, height: 2, op: "fill", format: "webp",
)
#assert.eq(slot.width, 4)
#assert.eq(slot.height, 2)
#document("index.html")[
  #html.img(src: slot.url, width: slot.width, height: slot.height, alt: "Example logo")
]
```

示例：裁成正方形并保留原图左边缘：

```typst site
#import "@tola/image:0.0.0": resize-image
#let edge = resize-image(
  "assets/logo.png",
  width: 2, height: 2, op: "fill", anchor: "left",
)
#assert.eq(edge.width, 2)
#assert.eq(edge.height, 2)
```

示例：指定底色并转换为 JPEG：

```typst site
#import "@tola/image:0.0.0": resize-image
// This source declares no alpha, so the background changes nothing here; a source with
// alpha needs one to encode as JPEG.
#let flat = resize-image(
  "assets/logo.png",
  width: 4, height: 2, op: "fit", format: "jpg", background: rgb("#ffffff"),
)
#assert.eq(flat.width, 4)
#assert.eq(flat.height, 2)
#assert(flat.url.ends-with(".jpg"))
```

示例：将 SVG 转为位图：

在站点根目录创建 `assets/`，将下面的 SVG 存为 `assets/mark.svg`。在 `site.typ` 中加入后面的 Typst 代码，再运行 `tola build`。Tola 会生成一张 12 × 12 像素的 PNG。

```svg assets/mark.svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <circle cx="12" cy="12" r="10" fill="#3569b0"/>
</svg>
```

```typst site
#import "@tola/image:0.0.0": resize-image
#let mark = resize-image("assets/mark.svg", width: 12, op: "fit-width", format: "png")
#assert.eq(mark.width, 12)
#assert.eq(mark.height, 12)
#assert(mark.url.ends-with(".png"))
```

### `source`

可传文件路径或图片字节。字符串路径从当前 Typst 文件所在目录算起。要把路径交给其他文件或包，请先用 `path()`。

### `anchor`

`fill` 裁剪时保留的位置。省略时居中。也可用 `top`、`bottom`、`left`、`right`、`top-left`、`top-right`、`bottom-left` 或 `bottom-right`。其他操作只接受 `center`。

### `background`

不透明的底色。源图带 alpha 且输出为 JPEG 时必填。合成前会转换为 sRGB。

### `filter`

缩放时用的重采样滤镜。省略时用 `lanczos3`，也可选 `nearest`、`triangle`、`catmull-rom` 或 `gaussian`。SVG 按目标尺寸渲染，滤镜对它无效。

### `format`

可选 `jpg`（或 `jpeg`）、`png`、`webp`。省略或设为 `auto` 时，有损且不带 alpha 的源图转成 JPEG，其余转成 PNG。把 SVG 转成位图时，须显式指定格式。

### `height`

目标高度，单位是像素，必须为正整数。`fit-height`、`scale`、`fit` 和 `fill` 必填。`fit-width` 会按宽高比计算高度。

### `op`

缩放方式。省略时用 `fill`，按指定宽高裁剪。要只指定一边并保持宽高比，用 `fit-width` 或 `fit-height`。

### `quality`

JPEG 接受 1–100，省略或传 `none` 时用 75。WebP 传入 0–100 时有损压缩，省略或传 `none` 时无损压缩。PNG 忽略此项。

### `width`

目标宽度，单位是像素，必须为正整数。`fit-width`、`scale`、`fit` 和 `fill` 必填。`fit-height` 会按宽高比计算宽度。

## `image-metadata`

读取图片尺寸、格式和透明度。可以据此决定缩放宽度，或在转成 JPEG 前设置背景色。

- `width`、`height`：按图片方向调整后的尺寸，单位是 CSS 像素。
- `format`：`jpg`、`png`、`webp`、`gif`、`bmp` 或 `svg`。`mime` 是对应的媒体类型。
- `has-alpha`：是否有透明信息。SVG 视为有 alpha。GIF 声明了透明色或有未覆盖的画布区域时，也返回 `true`。
- `is-lossy`：图片是否使用有损编码。

SVG 的长度按 96 DPI 换算，向上取整到整数像素。只给一边时，按 `viewBox` 的宽高比算出另一边。两边都未指定时，取 `viewBox` 的宽高。没有 `viewBox` 时，必须指定宽和高。

要在浏览器里引用原图，把文件加到 `[assets]`。

示例：读取图片元数据：

```typst site
#import "@tola/image:0.0.0": image-metadata
#let metadata = image-metadata("assets/logo.png")
#assert.eq(metadata.width, 8)
#assert.eq(metadata.height, 4)
#assert.eq(metadata.format, "png")
#assert.eq(metadata.has-alpha, false)
```

示例：限制预览宽度并保留透明区域：

```typst site
#import "@tola/image:0.0.0": image-metadata, resize-image
#let source = "assets/logo.png"
#let metadata = image-metadata(source)
// JPEG is the smaller choice, but it holds no alpha; keep PNG when the source declares any,
// and never scale past the source's own width.
#let width = if metadata.width > 640 { 640 } else { metadata.width }
#let preview = resize-image(
  source,
  width: width,
  op: "fit-width",
  format: if metadata.has-alpha { "png" } else { "jpg" },
)
#assert.eq(metadata.has-alpha, false)
#assert.eq(preview.width, 8)
#assert(preview.url.ends-with(".jpg"))
```

示例：读取 SVG 尺寸：

读取 `resize-image` 示例中保存的 `assets/mark.svg`。它的 `viewBox` 宽高都是 24，所以 `image-metadata` 返回 24 × 24。

```typst site
#import "@tola/image:0.0.0": image-metadata
#let mark = image-metadata("assets/mark.svg")
#assert.eq(mark.format, "svg")
#assert.eq(mark.width, 24)
#assert.eq(mark.height, 24)
#assert.eq(mark.has-alpha, true)
```

### `source`

可传文件路径或图片字节。字符串路径从当前 Typst 文件所在目录算起。要把路径交给其他文件或包，请先用 `path()`。

## `image-srcset`

将位图缩放到多个宽度，返回生成响应式 `img` 需要的值。

按页面布局给出宽度，例如 `widths: (480, 960)`。用 HTML 的 `sizes` 告诉浏览器图片在页面上占多宽。SVG 可用 `image()` 显示，或通过指定输出格式的 `resize-image` 转为位图。

`widths: auto`（默认）按原尺寸编码。给出列表时，宽度从小到大排序并去重。缩放保持宽高比，可以放大源图。

`url`、`width` 和 `height` 对应最宽的图片。`srcset` 列出每个 URL 和对应宽度，例如 `480w`。

要直接生成 `<img>`，用 `img`。自己写 HTML 时，可以用 `html.elem("img", attrs: ...)` 设置 `sizes`。这个属性支持视口单位，最后一项可以填无条件的默认宽度。

构建会把各宽度的图片写入 `_tola/images/`。源图、宽度和其他设置相同的调用共用一个文件。

示例：生成两种宽度的图片：

```typst site
#import "@tola/image:0.0.0": image-srcset
#let photo = image-srcset("assets/logo.png", widths: (2, 4))
#assert.eq(photo.width, 4)
#assert(photo.srcset.ends-with(" 4w"))
```

示例：将最宽的图片用作分享卡片封面：

将 PNG 放到 `assets/logo.png`，在 `tola.toml` 的 `[site]` 下设置 `origin = "https://example.test"`。把下面的代码保存为 `site.typ`，运行 `tola build --minify=false`。

```typst site
#import "@tola/image:0.0.0": image-srcset
#import "@tola/head:0.0.0": open-graph
#import "@tola/site:0.0.0": site
#let origin = site.origin
#let photo = image-srcset("assets/logo.png", widths: (480, 960))
#let entries = open-graph(
  title: "Field notes",
  url: site.url,
  images: ((url: origin + photo.url, alt: "Site logo"),),
)
#document("index.html", title: "Field notes")[
  #html.html[#html.head(entries.join()) #html.body[Field notes]]
]
```

`public/index.html` 的 `<head>` 中会出现下面的标签。文件名取决于图片和缩放设置。`photo.url` 已包含 `site.base-path`。

```html
<meta property="og:image" content="https://example.test/_tola/images/…webp">
```

### `source`

可传文件路径或图片字节。字符串路径从当前 Typst 文件所在目录算起。要把路径交给其他文件或包，请先用 `path()`。

### `background`

不透明的底色。源图带 alpha 且输出为 JPEG 时必填。合成前会转换为 sRGB。

### `format`

输出格式，取 `auto`、`jpg`（也可写 `jpeg`）、`png` 或 `webp`；默认 `webp`。

### `quality`

JPEG 接受 1–100，WebP 接受 0–100，省略时两者都用 75。要输出无损 WebP，传 `none`。PNG 忽略此项。

### `widths`

`auto`（默认，采用源图自身的宽度），或要生成的像素宽度列表；列表按升序排列并去掉重复值。

## `img`

生成响应式 `<img>`，让浏览器选合适尺寸的图片。

`widths` 决定生成哪些图片文件。`sizes` 告诉浏览器图片在页面上显示多宽，浏览器据此从 `srcset` 选图。例如，`"(max-width: 760px) 100vw, 720px"` 表示屏幕宽度不超过 760 像素时显示为全宽，其余情况显示为 720 CSS 像素。

`img` 接受位图。SVG 可以直接用 Typst 的 `image()`，也可以先用 `resize-image` 转成位图。

元素带有最宽图片的 `src`、像素 `width` 和 `height`，浏览器据此为图片预留空间。宽度去重后至少剩两个时才写入 `srcset` 和 `sizes`。`alt` 即使留空也会写出。装饰图片使用 `alt: ""`。

`attrs` 的值必须是字符串。`src`、`srcset`、`sizes`、`width`、`height` 和 `alt` 由函数设置，改写这些名字的大小写也不能传入。`loading` 与 `decoding` 默认 `lazy` 与 `async`，可在 `attrs` 里覆盖。其余属性（包括 `data-*`）原样写出。

`img` 用于 HTML 文档，生成 PDF 等分页文档时请用 Typst 的 `image`。

示例：在页面中放入响应式图片：

准备 `assets/logo.png`，将下面的代码保存为新站点的 `site.typ`。运行 `tola build`，首页的 `img` 会在 `srcset` 中列出两种宽度。把 `2` 和 `4` 换成页面布局需要的像素宽度。

```typst site
#import "@tola/image:0.0.0": img
#document("index.html")[#img("assets/logo.png", widths: (2, 4), alt: "Logo")]
```

示例：在最宽 720 像素的布局中优先加载首屏图片：

```typst site
#import "@tola/image:0.0.0": img
#document("index.html")[
  #img(
    "assets/logo.png",
    widths: (2, 4),
    alt: "Logo",
    sizes: "(max-width: 760px) 100vw, 720px",
    attrs: (loading: "eager", fetchpriority: "high"),
  )
]
```

### `alt`

替代文本；省略即写出空字符串，把图片标记为装饰。

### `attrs`

额外的 HTML 属性，值必须是字符串。例如，`(loading: "eager", fetchpriority: "high")` 可让首屏图片尽早加载。

### `background`

不透明的底色。源图带 alpha 且输出为 JPEG 时必填。合成前会转换为 sRGB。

### `format`

输出格式，取 `auto`、`jpg`（也可写 `jpeg`）、`png` 或 `webp`；默认 `webp`。

### `source`

可传文件路径或图片字节。字符串路径从当前 Typst 文件所在目录算起。要把路径交给其他文件或包，请先用 `path()`。

### `quality`

JPEG 接受 1–100，WebP 接受 0–100，省略时两者都用 75。要输出无损 WebP，传 `none`。PNG 忽略此项。

### `sizes`

告诉浏览器图片在页面中会显示多宽。省略时用 `100vw`，即视口全宽。去重后至少有两个输出宽度时才写入此属性。

### `widths`

`auto`（默认，采用源图自身的宽度），或要生成的像素宽度列表。
