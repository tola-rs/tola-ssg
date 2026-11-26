#show: package(
  summary: [Inspect image metadata, resize/convert images, and generate responsive `<img>` elements with `srcset`.],
  related: ("[assets]", "@tola-scripts/image"),
)

Generated images are written under `_tola/images/` and deduplicated automatically when identical calls share the same source and options. Use #ref("[assets]") if you also want to publish the untouched original image. Animated images are rejected.

#export(
  "resize-image",
  summary: [Resize or convert an image, publish it under `_tola/images/`, and return its URL and dimensions.],
  params: (
    source: [File path (relative to the calling `.typ` file, or a `path(...)` value) or image bytes.],
    width: [Positive target width in pixels. Required by `fit-width`, `scale`, `fit`, and `fill`.],
    height: [Positive target height in pixels. Required by `fit-height`, `scale`, `fit`, and `fill`.],
    op: [Resize operation: `"fill"` (default, crop to exact dimensions), `"fit"` (contain without upscaling), `"fit-width"`, `"fit-height"`, or `"scale"` (stretch).],
    anchor: [Crop anchor for `"fill"`: `"center"` (default), `"top"`, `"bottom"`, `"left"`, `"right"`, `"top-left"`, `"top-right"`, `"bottom-left"`, or `"bottom-right"`. Other modes accept only `"center"`.],
    format: [`"auto"` (default: JPEG for opaque lossy sources, PNG otherwise), `"jpg"` / `"jpeg"`, `"png"`, or `"webp"`. SVG inputs require an explicit raster format.],
    quality: [JPEG `1..100` (default `75`); WebP `0..100` (`none` for lossless). Ignored for PNG.],
    filter: [Raster resampling filter: `"lanczos3"` (default), `"nearest"`, `"triangle"`, `"catmull-rom"`, or `"gaussian"`. Ignored for SVG.],
    background: [Opaque background `color` composited under transparent pixels. Required when converting images with alpha to JPEG.],
  ),
  related: ("image-metadata", "image-srcset", "img"),
)[
  Returns `(url:, width:, height:, original-width:, original-height:, format:, mime:)` with `site.base-path` already applied to `url`. JPEG dimensions are limited to 65,535 px (4:2:0 chroma subsampling); WebP is limited to 16,383 px. SVG external images must use data URIs.

  #example(title: [Resize a cover image to WebP], check: "site")[
  ```typst
  #import "@tola/image:0.0.0": resize-image

  #let thumb = resize-image("assets/logo.png", width: 200, op: "fit-width", format: "webp")
  #document("index.html")[
    #html.img(src: thumb.url, width: str(thumb.width), height: str(thumb.height), alt: "Logo")
  ]
  ```
  ]
]

#export(
  "image-metadata",
  summary: [Read an image's orientation-adjusted pixel dimensions, format, MIME type, alpha, and lossiness.],
  params: (
    source: [File path or image bytes.],
  ),
  related: ("resize-image",),
)[
  Returns `(width:, height:, format:, mime:, has-alpha:, is-lossy:)` where `format` is one of `"jpg"`, `"png"`, `"webp"`, `"gif"`, `"bmp"`, or `"svg"`.
]

#export(
  "image-srcset",
  summary: [Generate multiple width variants of a raster image and return a ready-to-use `srcset` string plus fallback URL.],
  params: (
    source: [Raster image path or bytes.],
    widths: [`auto` (default, source width) or array of positive pixel widths (sorted and deduplicated).],
    format: [Output format: `"webp"` (default), `"auto"`, `"jpg"` / `"jpeg"`, or `"png"`.],
    quality: [JPEG `1..100` or WebP `0..100` (default `75`; pass `none` for lossless WebP). Ignored for PNG.],
    background: [Opaque background `color` required when converting transparent images to JPEG.],
  ),
  related: ("img", "resize-image", "@tola/head#open-graph"),
)[
  Returns `(url:, width:, height:, srcset:, variants:)` where `url` points to the widest generated variant (with `site.base-path`).
]

#export(
  "img",
  summary: [Emit a responsive HTML `<img>` element with width, height, and automatic `srcset`/`sizes`.],
  params: (
    source: [Raster image path or bytes. For SVG, use Typst's `image(...)` or `resize-image` first.],
    widths: [`auto` (default, source width) or array of positive pixel widths.],
    alt: [Alternative text (default `""`, marking decorative images). Always written to the `<img>` tag.],
    sizes: [CSS `sizes` hint (default `"100vw"`). Emitted when at least two distinct widths are generated.],
    format: [Output format: `"webp"` (default), `"auto"`, `"jpg"` / `"jpeg"`, or `"png"`.],
    quality: [JPEG `1..100` or WebP `0..100` (default `75`; `none` for lossless WebP). Ignored for PNG.],
    background: [Opaque background `color` required when converting transparent images to JPEG.],
    attrs: [Extra string-valued HTML attributes. Defaults `loading: "lazy"` and `decoding: "async"` can be overridden here; managed attributes (`src`, `srcset`, `sizes`, `width`, `height`, `alt`) are rejected.],
  ),
  related: ("image-srcset", "resize-image"),
)[
  Intended for HTML export; use Typst's built-in `image(...)` for PDF or paged targets.

  #example(title: [Render a responsive hero image with eager loading], check: "site")[
  ```typst
  #import "@tola/image:0.0.0": img

  #document("index.html")[
    #img(
      "assets/logo.png",
      widths: (480, 960),
      alt: "Hero banner",
      sizes: "(max-width: 760px) 100vw, 720px",
      attrs: (loading: "eager", fetchpriority: "high"),
    )
  ]
  ```
  ]
]
