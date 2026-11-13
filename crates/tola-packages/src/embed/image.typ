// @tola/image:0.0.0 - image metadata, single images resized on request, and responsive sets

/// Resize an image and publish the result under `_tola/images/`, even when you leave the
/// return value unused. Pass its `url`, `width`, and `height` to `html.img`. The URL includes
/// `site.base-path`. The result also includes the source dimensions in `original-width` and
/// `original-height`. All dimensions are in pixels.
///
/// You can also pass image bytes. Use `path()` before passing a relative path to a function
/// in another file or package.
///
/// `scale` sets both dimensions independently. `fit-width` and `fit-height` derive the other
/// dimension from the aspect ratio and round it to the nearest pixel, with a minimum of one.
/// `fit` keeps the whole image inside the requested box and leaves it at its original size
/// if it already fits. `fill` crops to the requested size. Every operation except `fit` can
/// enlarge the image.
///
/// #### Image limits
///
/// JPEG supports up to 65,535 pixels per axis and uses 4:2:0 chroma subsampling. WebP supports
/// up to 16,383 pixels per axis. PNG preserves more detail in coloured text and fine lines.
/// Images embedded in an SVG must use data URIs. SVG text uses the compiler's default fonts.
/// Convert text to paths to keep its shapes. Animated input returns an error.
///
/// Use `[assets]` to publish the original too. Run `tola check --json` to inspect generated
/// image paths and media types.
///
/// Example: Publish a 640-pixel preview:
///
/// Put a PNG at `assets/logo.png`, save this as `site.typ`, and run `tola build`.
/// Tola writes a 640-pixel-wide image to `public/_tola/images/` and places it on the home page
/// at `public/index.html`. The height follows the source aspect ratio.
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let preview = resize-image("assets/logo.png", width: 640, op: "fit-width")
/// #document("index.html")[
///   #html.img(src: preview.url, width: preview.width, height: preview.height, alt: "Site logo")
/// ]
/// ```
///
/// Example: Check the output and source dimensions:
///
/// The following size assertions use an 8 × 4 PNG at `assets/logo.png`, relative to the
/// entry file. With your own image, use its dimensions for the `original-*` assertions.
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
/// Example: Derive the height from the width:
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let resized = resize-image("assets/logo.png", width: 4, op: "fit-width")
/// #assert.eq(resized.width, 4)
/// #assert.eq(resized.height, 2)
/// ```
///
/// Example: Resize into a fixed layout slot:
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
/// Example: Crop a square and keep the left edge:
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
/// Example: Convert to JPEG with a background colour:
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
/// Example: Rasterize an SVG:
///
/// Create `assets/` in the site root and save this SVG as `assets/mark.svg`. Add the Typst code to
/// `site.typ` and run `tola build`. Tola generates a 12 × 12 PNG.
///
/// ```svg assets/mark.svg
/// <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
///   <circle cx="12" cy="12" r="10" fill="#3569b0"/>
/// </svg>
/// ```
///
/// ```typst site
/// #import "@tola/image:0.0.0": resize-image
/// #let mark = resize-image("assets/mark.svg", width: 12, op: "fit-width", format: "png")
/// #assert.eq(mark.width, 12)
/// #assert.eq(mark.height, 12)
/// #assert(mark.url.ends-with(".png"))
/// ```
///
/// Related: image-metadata, @tola/address, @tola/site, @tola/head
#import "@tola/host:0.0.0": resize-image

/// Read an image's size, format, and transparency. Use the size to limit an image resize,
/// or check `has-alpha` before converting to JPEG.
///
/// `source` accepts the same inputs as `resize-image`. The result has these fields:
///
/// - `width` and `height`: size in CSS pixels after accounting for image orientation.
/// - `format`: `jpg`, `png`, `webp`, `gif`, `bmp`, or `svg`. `mime` gives the media type.
/// - `has-alpha`: whether the image can contain transparency. This is true for SVG and
///   for GIF with a declared transparent color or an uncovered part of the canvas.
/// - `is-lossy`: whether the image uses lossy encoding.
///
/// SVG lengths are converted at 96 DPI and rounded up to whole pixels. If only one
/// dimension is specified, Tola calculates the other from the `viewBox` aspect ratio.
/// With neither dimension, Tola uses the `viewBox` size. Without a `viewBox`, both
/// dimensions are required.
///
/// To use the original image in the browser, add it to `[assets]`.
///
/// Example: Read an image's metadata:
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
/// Example: Limit the preview width and keep transparency:
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
/// Example: Read the dimensions of an SVG:
///
/// Use `assets/mark.svg` from the `resize-image` example. Its `viewBox` is 24 × 24,
/// so `image-metadata` returns that size.
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
/// Related: resize-image, @tola/address, @tola/head
#import "@tola/host:0.0.0": image-metadata

/// Resize an image to several widths and return values for a responsive `<img>`.
///
/// Pass the widths your layout needs, such as `widths: (480, 960)`. Set HTML `sizes` to
/// tell the browser how wide the image appears in the layout.
///
/// This function accepts raster images. To display an SVG, use `image()`.
/// To convert it to a bitmap, call `resize-image` with a raster format.
///
/// With `widths: auto` (the default), the image is encoded at its original size. A list of widths
/// is sorted from smallest to largest, and repeated widths are counted once. Resizing keeps the
/// aspect ratio and can enlarge the image.
///
/// `url`, `width`, and `height` describe the widest image. `srcset` lists each URL with
/// its width, such as `480w`.
///
/// Use `img` to create the element directly. To write your own `sizes` attribute, use
/// `html.elem("img", attrs: ...)`. It accepts CSS viewport units and a final width without
/// a media condition.
///
/// A build publishes the resized files under `_tola/images/`. Calls with the same source,
/// width, and settings share the same file.
///
/// Example: Resize one image to two widths:
///
/// ```typst site
/// #import "@tola/image:0.0.0": image-srcset
/// #let photo = image-srcset("assets/logo.png", widths: (2, 4))
/// #assert.eq(photo.width, 4)
/// #assert(photo.srcset.ends-with(" 4w"))
/// ```
///
/// Example: Use the widest image in the page's share card:
///
/// Put a PNG at `assets/logo.png`
/// and set `origin = "https://example.test"` under `[site]` in `tola.toml`. Save the
/// example as `site.typ` and run `tola build --minify=false`.
///
/// ```typst site
/// #import "@tola/image:0.0.0": image-srcset
/// #import "@tola/head:0.0.0": open-graph
/// #import "@tola/site:0.0.0": site
/// #let origin = site.origin
/// #let photo = image-srcset("assets/logo.png", widths: (480, 960))
/// #let entries = open-graph(
///   title: "Field notes",
///   url: site.url,
///   images: ((url: origin + photo.url, alt: "Site logo"),),
/// )
/// #document("index.html", title: "Field notes")[
///   #html.html[#html.head(entries.join()) #html.body[Field notes]]
/// ]
/// ```
///
/// In `public/index.html`, the `<head>` includes the following tag. The filename depends on
/// the image and resize settings. `photo.url` already includes `site.base-path`.
///
/// ```html
/// <meta property="og:image" content="https://example.test/_tola/images/…webp">
/// ```
///
/// Related: img, resize-image, image-metadata, @tola/address, @tola/head
#import "@tola/host:0.0.0": image-srcset

/// Place one image at several widths, as a responsive `<img>`.
///
/// `widths` lists the image files to generate. `sizes` tells the browser how wide the
/// image will appear, so it can choose a file from `srcset`. For example,
/// `"(max-width: 760px) 100vw, 720px"` tells the browser to use the full viewport
/// width up to 760 pixels, and 720 CSS pixels on larger screens.
///
/// The element carries the widest image's `src`, its pixel `width` and `height` so the page
/// reserves space, and `srcset` with `sizes` only when at least two distinct widths remain. `alt` is
/// written even when you leave it empty. Use `alt: ""` for a decorative image.
///
/// Use `attrs` for extra HTML attributes with string values. The function sets `src`,
/// `srcset`, `sizes`, `width`, `height`, and `alt` itself. These names are reserved
/// regardless of case. To override `loading: "lazy"` or `decoding: "async"`, set
/// those attributes in `attrs`.
///
/// The element is HTML: call it from an HTML document. A paged document uses `image` instead.
///
/// This function accepts raster images. For SVG, use `image()` or convert it first
/// with `resize-image` and a raster format.
///
/// Example: Add a responsive image to a page:
///
/// Place a PNG at `assets/logo.png` and save this as `site.typ` in a new site. Run
/// `tola build`. The home page has an `img` with a `srcset` for both widths. Replace
/// `2` and `4` with the widths your layout needs.
///
/// ```typst site
/// #import "@tola/image:0.0.0": img
/// #document("index.html")[#img("assets/logo.png", widths: (2, 4), alt: "Logo")]
/// ```
///
/// Example: Prioritize a hero image in a layout up to 720 pixels wide:
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
/// Related: image-srcset, resize-image, image-metadata, @tola/head
#import "@tola/host:0.0.0": img
