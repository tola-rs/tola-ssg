// @tola/image:0.0.0 - image metadata, and images resized on request

/// Request the derivative a build publishes, and return its URL and dimensions.
///
/// `path` names the input file: either a string resolved against this call, or a path you
/// already resolved with `path()`. Resolve a string before you pass it to another file or
/// package. Pass the file's own path even when an asset declaration publishes it; the
/// declaration's URL is for browsers. `width` and `height` are positive pixel counts.
///
/// `op` names the operation: `scale`, `fit-width`, `fit-height`, `fit`, or `fill` (the default).
/// It decides which of the two dimensions you must pass:
///
/// - `scale` needs both, and may change the aspect ratio.
/// - `fit-width` needs `width`, and `fit-height` needs `height`. Both keep the aspect ratio.
/// - `fit` needs both, fits the image inside that box, and keeps the source size when the image
///   already fits.
/// - `fill` needs both, and crops the image to that size around its center.
///
/// A dimension you omit is `none`. `fit-width` uses only `width`, and `fit-height` uses only
/// `height`. Every operation except `fit` can enlarge the source. A derived dimension rounds to
/// the nearest pixel, with a minimum of one pixel.
///
/// `format` is `auto`, `jpg`, `png`, or `webp`. `auto` chooses JPEG for a lossy source without
/// alpha, and PNG for everything else, including GIF. JPEG quality runs from 1 to 100 and
/// defaults to 75. WebP is lossless until you pass `quality` (0 to 100); PNG is always lossless.
/// A valid `quality` affects only JPEG and WebP. Output dimensions cannot exceed 65,535 pixels
/// per axis for JPEG, or 16,383 for WebP. `quality: none` uses the format's default.
///
/// AVIF output is not supported yet; choose `webp`, `png`, `jpg`, or `auto` instead.
///
/// `filter` names the resampling kernel: `nearest`, `triangle`, `catmull-rom`, `gaussian`, or
/// `lanczos3` (the default). The kernel applies only when the crop and the output differ in size.
/// `background` accepts an opaque Typst color, which Typst converts to sRGB before compositing.
/// JPEG output needs `background` when the source's `has-alpha` is true; a source without alpha
/// encodes directly. A `filter` or `background` with no effect resolves to the same derivative as
/// omitting it.
///
/// The result carries the derivative's published `url`, its `width` and `height`, and the
/// source's own `original-width` and `original-height`. The call requests the derivative whether
/// or not you use the returned URL and dimensions: a build publishes every derivative its
/// converged evaluation requested, and an editor check reports diagnostics only. This call
/// publishes the derivative alone; the source image keeps whatever the asset declaration
/// publishes for it. `resize-image` refuses animated images and SVG sources. The URL includes
/// `site.base-path`, ready for `html.img(src: resized.url, ...)`. Pass the integer pixel
/// dimensions straight to `html.img`'s `width` and `height`.
///
/// A build adds each requested derivative to the site's output graph, under `_tola/images/` and
/// a name derived from the source bytes and the recipe; equivalent requests land on one output.
/// The returned `url` is the ordinary way to link it. When only an output path is at hand,
/// `output-to-url` turns it into the browser URL; `tola inspect outputs` lists every published
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
/// Related: image-metadata, @tola/address, @tola/site, @tola/web
#import "@tola/host:0.0.0": resize-image

/// Inspect an image, and read its metadata.
///
/// `path` behaves as in `resize-image`. The result carries `width`, `height`, `format`, `mime`,
/// `has-alpha`, and `is-lossy`. The dimensions account for the image's orientation. `format` is
/// `jpg`, `png`, `webp`, `gif`, `bmp`, or `svg`. Inspection reads what the file declares about
/// itself, so `has-alpha` counts an unused GIF transparency declaration and an uncovered GIF
/// canvas as alpha. The original file's URL comes from its own asset declaration: declare the
/// file as an asset when it needs a URL.
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
/// Related: resize-image, @tola/address, @tola/web
#import "@tola/host:0.0.0": image-metadata
