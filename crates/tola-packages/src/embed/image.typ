// @tola/image:0.0.0 - image metadata, single images resized on request, and responsive sets

/// Request the derivative a build publishes, and return its URL and dimensions.
///
/// `source` names the input image: a path string resolved against this call, a path you already
/// resolved with `path()`, or raw image bytes. Resolve a string before you pass it to another file
/// or package. Pass the file's own path even when an asset declaration publishes it; the
/// declaration's URL is for browsers. `width` and `height` are positive pixel counts.
///
/// `op` names the operation: `scale`, `fit-width`, `fit-height`, `fit`, or `fill` (the default).
/// It decides which of the two dimensions you must pass:
///
/// - `scale` needs both, and may change the aspect ratio.
/// - `fit-width` needs `width`, and `fit-height` needs `height`. Both keep the aspect ratio.
/// - `fit` needs both, fits the image inside that box, and keeps the source size when the image
///   already fits.
/// - `fill` needs both, crops the image to that size, and keeps the part named by `anchor` (the
///   center by default).
///
/// `anchor` decides which part `fill` keeps: `center` (the default), `top`, `bottom`, `left`,
/// `right`, `top-left`, `top-right`, `bottom-left`, or `bottom-right`. Every other operation keeps
/// the whole source, so it refuses any `anchor` but `center`.
///
/// A dimension you omit is `none`. `fit-width` uses only `width`, and `fit-height` uses only
/// `height`. Every operation except `fit` can enlarge the source. A derived dimension rounds to
/// the nearest pixel, with a minimum of one pixel.
///
/// `format` is `auto`, `jpg` (or `jpeg`), `png`, or `webp`. `auto` chooses JPEG for a lossy source
/// without alpha, and PNG for everything else, including GIF. JPEG quality runs from 1 to 100 and
/// defaults to 75. WebP is lossless until you pass `quality` (0 to 100); PNG is always lossless.
/// A valid `quality` affects only JPEG and WebP. Output dimensions cannot exceed 65,535 pixels
/// per axis for JPEG, or 16,383 for WebP. JPEG halves colour resolution (4:2:0); use PNG for
/// coloured text and fine graphics. `quality: none` uses the format's default.
///
/// An SVG source is rasterized at the requested pixel size and needs an explicit raster `format`:
/// `auto` cannot choose one. Its text uses the compiler's default fonts, and linked images must be
/// data URIs. Outline its text when the raster must not depend on those fonts.
///
/// AVIF output is not supported; choose `webp`, `png`, `jpg`, or `auto` instead.
///
/// `filter` names the resampling kernel: `nearest`, `triangle`, `catmull-rom`, `gaussian`, or
/// `lanczos3` (the default). The kernel applies only when the crop and the output differ in size;
/// an SVG is rendered at the target size, so `filter` has no effect on it. `background` accepts an
/// opaque Typst color, which Typst converts to sRGB before compositing. JPEG output needs
/// `background` when the source's `has-alpha` is true; a source without alpha encodes directly. A
/// `filter` or `background` with no effect resolves to the same derivative as omitting it.
///
/// The result carries the derivative's published `url`, its `width` and `height`, and the
/// source's own `original-width` and `original-height`. The call requests the derivative whether
/// or not you use the returned URL and dimensions: a build publishes every derivative its
/// converged evaluation requested, and an editor check reports diagnostics only. This call
/// publishes the derivative alone; the source image keeps whatever the asset declaration
/// publishes for it. `resize-image` refuses animated images. The URL includes
/// `site.base-path`, ready for `html.img(src: resized.url, ...)`. Pass the integer pixel
/// dimensions straight to `html.img`'s `width` and `height`.
///
/// A build adds each requested derivative to the site's output graph, under `_tola/images/` and
/// a name derived from the source bytes and the recipe; equivalent requests land on one output.
/// The returned `url` is the ordinary way to link it. When only an output path is at hand,
/// `output-url` turns it into the browser URL; `tola inspect outputs` lists every published
/// output with its `producer` and `media-type`.
///
/// Example - resize to an exact width and height:
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let resized = resize-image("assets/logo.png", width: 4, height: 4)
/// #assert.eq(resized.width, 4)
/// #assert.eq(resized.height, 4)
/// #assert.eq(resized.original-width, 8)
/// #assert.eq(resized.original-height, 4)
/// ```
///
/// Example - derive the height from the width:
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let resized = resize-image("assets/logo.png", width: 4, op: "fit-width")
/// #assert.eq(resized.width, 4)
/// #assert.eq(resized.height, 2)
/// ```
///
/// Example - resize into a fixed layout slot:
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let slot = resize-image(
///   "assets/logo.png",
///   width: 4, height: 2, op: "fill", format: "webp",
/// )
/// #assert.eq(slot.width, 4)
/// #assert.eq(slot.height, 2)
/// #document("index.html")[
///   #html.img(src: slot.url, width: slot.width, height: slot.height, alt: "Example logo")
/// ]
/// ```
///
/// Example - keep the left edge of the source when filling a slot:
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let edge = resize-image(
///   "assets/logo.png",
///   width: 2, height: 2, op: "fill", anchor: "left",
/// )
/// #assert.eq(edge.width, 2)
/// #assert.eq(edge.height, 2)
/// ```
///
/// Example - flatten transparency onto an opaque background before JPEG encoding:
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// // This source declares no alpha, so the background changes nothing here; a source with
/// // alpha needs one to encode as JPEG.
/// #let flat = resize-image(
///   "assets/logo.png",
///   width: 4, height: 2, op: "fit", format: "jpg", background: rgb("#ffffff"),
/// )
/// #assert.eq(flat.width, 4)
/// #assert.eq(flat.height, 2)
/// #assert(flat.url.ends-with(".jpg"))
/// ```
///
/// Example - rasterize an SVG for a bitmap consumer:
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let mark = resize-image("assets/mark.svg", width: 12, op: "fit-width", format: "png")
/// #assert.eq(mark.width, 12)
/// #assert.eq(mark.height, 12)
/// #assert(mark.url.ends-with(".png"))
/// ```
///
/// Related: image-metadata, @tola/address, @tola/site, @tola/web
#import "@tola/host:0.0.0": resize-image

/// Inspect an image, and read its metadata.
///
/// `source` behaves as in `resize-image`. The result carries `width`, `height`, `format`, `mime`,
/// `has-alpha`, and `is-lossy`. The dimensions account for the image's orientation. `format` is
/// `jpg`, `png`, `webp`, `gif`, `bmp`, or `svg`. Dimensions are CSS pixels; an SVG reports its
/// intrinsic size at 96 DPI, so a size in points converts at 4/3 (`256pt` reports `341`). A vector
/// without `width`/`height` reports its `viewBox` size, where a browser falls back to 300×150. An
/// SVG counts as having alpha. Inspection reads what the file declares about itself, so `has-alpha`
/// counts an unused GIF transparency declaration and an uncovered GIF canvas as alpha. The
/// original file's URL comes from its own asset declaration: declare the file as an asset when it
/// needs a URL.
///
/// Read it before resizing when the recipe depends on the source: `has-alpha` decides whether
/// JPEG needs a `background`, and `width` decides the target size.
///
/// Example - read an image's metadata:
///
/// ```typst site
/// #import "@tola/image:0.0.0": image-metadata
/// #let metadata = image-metadata("assets/logo.png")
/// #assert.eq(metadata.width, 8)
/// #assert.eq(metadata.height, 4)
/// #assert.eq(metadata.format, "png")
/// #assert.eq(metadata.has-alpha, false)
/// ```
///
/// Example - branch on the source's own dimensions and alpha:
///
/// ```typst site
/// #import "@tola/image:0.0.0": image-metadata, resize-image
/// #let source = "assets/logo.png"
/// #let metadata = image-metadata(source)
/// // JPEG is the smaller choice, but it holds no alpha; keep PNG when the source declares any,
/// // and never scale past the source's own width.
/// #let width = if metadata.width > 640 { 640 } else { metadata.width }
/// #let preview = resize-image(
///   source,
///   width: width,
///   op: "fit-width",
///   format: if metadata.has-alpha { "png" } else { "jpg" },
/// )
/// #assert.eq(metadata.has-alpha, false)
/// #assert.eq(preview.width, 8)
/// #assert(preview.url.ends-with(".jpg"))
/// ```
///
/// Example - read a vector's intrinsic size before rasterizing it:
///
/// ```typst site
/// #import "@tola/image:0.0.0": image-metadata
/// #let mark = image-metadata("assets/mark.svg")
/// #assert.eq(mark.format, "svg")
/// #assert.eq(mark.width, 24)
/// #assert.eq(mark.height, 24)
/// #assert.eq(mark.has-alpha, true)
/// ```
///
/// Related: resize-image, @tola/address, @tola/web
#import "@tola/host:0.0.0": image-metadata

/// Derive one image at several widths, and return what a responsive `<img>` needs.
///
/// This is the route for photographs: give it the widths your layout needs, and it requests one
/// derivative per width, exactly as `resize-image` does for a single one. `sizes` stays yours to
/// write — nothing here can know the width a browser will end up using.
///
/// An SVG source is refused: a vector has no `w` descriptor. Use `image()` for vector display, or
/// `resize-image` with an explicit raster format for a bitmap.
///
/// `widths` is `auto` by default, which derives the source's own width: a call with no widths
/// converts the format without resampling. A list of widths is sorted ascending and duplicates
/// collapse, so a call that asks for one width twice still derives it once. Every derivative uses
/// `fit-width` and keeps the source's aspect ratio, and a width larger than the source enlarges it,
/// as `resize-image` does.
///
/// The result carries one `<img>`'s values: `url` is the widest derivative, `srcset` lists every
/// derivative with its `w` descriptor, and `width` and `height` describe the widest one, so the page
/// can reserve the right space before the image loads. `format` defaults to `webp` and `quality` to
/// 75, because this route serves the page; `quality: none` selects lossless WebP.
///
/// Write these values with `html.elem`: a `sizes` list uses viewport units and ends with an entry
/// that has no media condition, which the typed `html.img` cannot express. The HTML layer parses and
/// rewrites the `srcset` URLs on its own. `img` below performs this assembly for one element.
///
/// Every derivative is requested on its own, so a build publishes one output per distinct width
/// under `_tola/images/`, and equivalent requests land on one output: two calls whose widths overlap
/// share those derivatives. An editor check reports diagnostics only, as with `resize-image`.
///
/// Example - derive one image at two widths:
///
/// ```typst site
/// #import "@tola/image:0.0.0": image-srcset
/// #let photo = image-srcset("assets/logo.png", widths: (2, 4))
/// #assert.eq(photo.width, 4)
/// #assert(photo.srcset.ends-with(" 4w"))
/// ```
///
/// Example - record the widest derivative as a page's social image:
///
/// ```typst site
/// #import "@tola/image:0.0.0": image-srcset
/// #let photo = image-srcset("assets/logo.png", widths: (2, 4))
/// #document("index.html")[
///   #html.elem("meta", attrs: (property: "og:image", content: photo.url))
/// ]
/// ```
///
/// Related: img, resize-image, image-metadata, @tola/address, @tola/web
#import "@tola/host:0.0.0": image-srcset

/// Place one image at several widths, as a responsive `<img>`.
///
/// `source` behaves as in `resize-image`. `widths` is `auto`, which derives the source's own width,
/// or a list of pixel widths to derive; `sizes` says how wide the element is displayed and defaults
/// to `100vw`, the browser's own assumption. Pass the width your layout reserves, such as
/// `"(max-width: 760px) 100vw, 720px"`, whenever the element is not full-width.
///
/// The element carries the widest derivative's `src`, its pixel `width` and `height` so the page
/// reserves space, and `srcset` with `sizes` only when more than one width was asked for. `alt` is
/// written even when you leave it empty: an omitted `alt` is invalid HTML, while `alt: ""` marks a
/// decorative image. `format` and `quality` default to `webp` and 75, as in `image-srcset`.
///
/// Attributes this function owns — `src`, `srcset`, `sizes`, `width`, `height`, and `alt` — cannot
/// also appear in `attrs`. `loading` and `decoding` default to `lazy` and `async` and may be
/// overridden there. Every other attribute, including `data-*`, passes through unchanged.
///
/// The element is HTML: call it from an HTML document. A paged document uses `image` instead.
///
/// An SVG source is refused for the same reason as `image-srcset`; use `image()` or
/// `resize-image` with an explicit raster format.
///
/// Example - one image at two widths:
///
/// ```typst site
/// #import "@tola/image:0.0.0": img
/// #document("index.html")[#img("assets/logo.png", widths: (2, 4), alt: "Logo")]
/// ```
///
/// Example - a layout that is at most 720 pixels wide, with a top-priority hero image:
///
/// ```typst site
/// #import "@tola/image:0.0.0": img
/// #document("index.html")[
///   #img(
///     "assets/logo.png",
///     widths: (2, 4),
///     alt: "Logo",
///     sizes: "(max-width: 760px) 100vw, 720px",
///     attrs: (loading: "eager", fetchpriority: "high"),
///   )
/// ]
/// ```
///
/// Related: image-srcset, resize-image, image-metadata, @tola/web
#import "@tola/host:0.0.0": img
