#show: script(
  summary: [Inspect image metadata or save resized and converted copies from the command line.],
)

#command(
  "main",
  summary: [List subcommands (`metadata` and `resize`).],
)

#command(
  "metadata",
  summary: [Print image dimensions (`width`, `height`), `format`, and alpha support (`has-alpha`) as JSON.],
  params: (
    source: [Image file path, resolved from the current working directory.],
  ),
)[
  #example(title: [Inspect a local PNG])[
    ```sh
    tola nu @tola-scripts/image metadata photo.png
    ```
  ]
]

#command(
  "resize",
  summary: [Resize, crop, or convert a local image (including SVG) into a new output file or raw bytes.],
  params: (
    source: [Source image path (raster or SVG).],
    "--output": [Destination path for a new file inside an existing directory. Prints resulting image metadata as JSON. Omit to return raw image bytes.],
    "--width": [Target width in pixels (positive integer). Required by `fit-width`, `scale`, `fit`, and `fill`.],
    "--height": [Target height in pixels (positive integer). Required by `fit-height`, `scale`, `fit`, and `fill`.],
    "--op": [Resize mode: `fit-width`, `fit-height` (preserve aspect ratio from one side), `fit` (contain without upscaling), `fill` (crop to exact size), or `scale` (stretch to exact size). When omitted, one dimension preserves aspect ratio, two dimensions crop, and no dimensions keep the original size.],
    "--anchor": [Crop focal point for `fill`. Defaults to `center`; also accepts `top`, `bottom`, `left`, `right`, `top-left`, `top-right`, `bottom-left`, and `bottom-right`. Other modes accept only `center`.],
    "--format": [Output format: `png`, `webp`, `jpg`, `jpeg`, or `auto`. For raster inputs, `auto` selects JPEG for opaque lossy sources and PNG otherwise. SVG inputs require an explicit raster format.],
    "--quality": [Encoding quality: `1..100` for JPEG (default `75`), `0..100` for WebP (lossless when omitted). Ignored for PNG.],
    "--filter": [Resampling filter for raster inputs: `lanczos3` (default), `nearest`, `triangle`, `catmull-rom`, or `gaussian`.],
    "--background": [Opaque `#RRGGBB` fill color for transparent pixels (for example `'#ffffff'`). Required when converting images with alpha to JPEG.],
  ),
)[
  Any external images referenced inside an SVG must be embedded as data URIs. Site hooks automatically inherit the site's #ref("[typst.fonts]") configuration when rendering SVG text; for standalone runs, pass `--config tola.toml` before the script name.

  #example(title: [Create a width-constrained WebP preview])[
    ```sh
    tola nu @tola-scripts/image resize photo.png --width 640 --format webp --output preview.webp
    ```
  ]

  #example(title: [Crop a square thumbnail anchored to the top])[
    ```sh
    tola nu @tola-scripts/image resize photo.png --width 160 --height 160 --anchor top --format webp --output square.webp
    ```
  ]

  #example(title: [Rasterize an SVG using site fonts])[
    ```sh
    tola nu --config tola.toml @tola-scripts/image resize wordmark.svg --width 240 --format png --output wordmark.png
    ```
  ]
]
