# `@tola-scripts/image`

Check an image’s size and format, or save a resized or converted copy.

## `main`

Run `tola nu @tola-scripts/image` to list the subcommands. Use `metadata` to inspect a file and `resize` to make a new one.

## `metadata`

Print JSON with the image’s `width` and `height` in pixels, its `format`, and whether it supports transparency (`has-alpha`).

Example: Inspect a PNG:

Put a PNG named `photo.png` in your current directory and run:

```sh
tola nu @tola-scripts/image metadata photo.png
```

### Parameters

- `source`: Path to the image file, relative to the directory where you run the command.

## `resize`

Resize or convert a local image and save it under a new filename. For SVG input, choose an output format such as PNG or WebP. Any images referenced by the SVG must be embedded as data URIs.

Example: Make a WebP preview:

From the directory containing `photo.png`, run:

```sh
tola nu @tola-scripts/image resize photo.png --width 640 --format webp --output preview.webp
```

This saves `preview.webp` at 640 pixels wide and calculates its height from the source aspect ratio.

Example: Crop a square thumbnail:

To fill a square thumbnail, pass both dimensions. Add `--anchor top` to keep the top of the photo when cropping:

```sh
tola nu @tola-scripts/image resize photo.png --width 160 --height 160 --anchor top --format webp --output square.webp
```

This saves a 160 × 160 WebP as `square.webp` in the same directory.

Example: Turn an SVG into a PNG:

Save this as `favicon.svg` in your current directory:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <circle cx="16" cy="16" r="14" fill="#3569b0"/>
</svg>
```

From that directory, run:

```sh
tola nu @tola-scripts/image resize favicon.svg --width 64 --format png --output favicon-64.png
```

This saves a 64 × 64 PNG.

Example: Use site fonts in an SVG:

Put `wordmark.svg` containing text next to `tola.toml`. Set its `font-family` to the
family name listed by `tola inspect fonts`. A site hook already uses the site's
`[typst.fonts]` settings. For a standalone run, pass the site's configuration explicitly:

```sh
tola nu --config tola.toml @tola-scripts/image resize wordmark.svg --width 240 --format png --output wordmark.png
```

This writes `wordmark.png` at 240 pixels wide, keeping the source aspect ratio.
If a hook reuses its outputs during development, list the SVG
and site font files it reads in `rerun-on`. Use `dev = "run"` if the output depends
on system fonts.

### Parameters

- `source`: Path to an image file, including SVG.
- `--output`: Save to a new file in an existing directory and print its image information as JSON. Omit the option to return the image bytes.
- `--width`: Target width in pixels, as a positive integer. Required for `fit-width`, `scale`, `fit`, and `fill`. `fit-height` calculates the width from the source.
- `--height`: Target height in pixels, as a positive integer. Required for `fit-height`, `scale`, `fit`, and `fill`. `fit-width` calculates the height from the source.
- `--op`: Use `fit-width` or `fit-height` to set one side and keep the aspect ratio. Use `fit` to fit the whole image within both dimensions without enlarging it, `fill` to crop to an exact size, or `scale` to stretch to an exact size. If omitted, one dimension preserves the aspect ratio, two dimensions crop, and no dimensions keep the original size.
- `--anchor`: Where `fill` crops. The default is `center`. Use `top`, `bottom`, `left`, `right`, `top-left`, `top-right`, `bottom-left`, or `bottom-right` to keep an edge or a corner. Other operations accept only `center`.
- `--format`: Choose `png`, `webp`, `jpg`, or `jpeg`. For raster inputs, `auto` uses JPEG for a lossy source without alpha and PNG otherwise. SVG requires an explicit format. Choose a filename extension that matches the output format.
- `--quality`: JPEG accepts 1–100 and defaults to 75. WebP accepts 0–100 and is lossless if you omit the option. PNG ignores it.
- `--filter`: Choose the resampling filter for raster inputs. If omitted, use `lanczos3`. Other choices are `nearest`, `triangle`, `catmull-rom`, and `gaussian`.
- `--background`: Fill transparency with a color such as `--background '#ffffff'` for white. Use six hexadecimal digits after `#`. Required when converting an image with alpha to JPEG.
