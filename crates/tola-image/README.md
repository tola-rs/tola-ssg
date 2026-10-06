# tola-image

Deterministic image processing: bytes in, encoded variant bytes out. The same source bytes and
recipe produce the same output bytes across platforms and thread counts.

## Read metadata

```rust
use tola_image::{ImageFormat, inspect};

let source = std::fs::read("photo.jpg")?;
let metadata = inspect(&source)?;

assert_eq!(metadata.format, ImageFormat::Jpeg);
println!("{}×{}", metadata.width, metadata.height);
```

`inspect` reads only the container, never decoding pixels. `ImageMetadata` reports display
dimensions (EXIF orientation already applied), the format, whether the source has transparency,
and whether it is lossy.

Containers: JPEG, PNG, WebP, GIF (still frames only), BMP (its embedded colour profile is read),
and SVG (measured, never resized). **AVIF is not supported yet.**

Colour declarations (ICC, gamma, sRGB) are resolved by the container's own precedence and
converted to sRGB.

## Resize and encode

```rust
use tola_image::{
    ImageRecipe, NO_CANCELLATION, ResizeOperation, ResizeOptions,
    render,
};

let recipe = ImageRecipe::resolve(
    &metadata,
    ResizeOptions {
        width: Some(640),
        operation: ResizeOperation::FitWidth,
        ..ResizeOptions::default()
    },
)?;

let encoded = render(&source, &recipe, &NO_CANCELLATION)?;
```

`ImageRecipe::resolve` resolves a request against the source's metadata; when the request cannot
work, the error says what to change. `ResizeOptions::default()` selects `Fill`, `Auto`, and
`Lanczos3`, without quality or background overrides. Supply both dimensions for the default
operation, or select `FitWidth` / `FitHeight` and supply its one required dimension.
A recipe has three parts:

- **Dimensions and operation** (`ResizeOperation`): `Scale` for the exact size (stretching when
  the aspect ratio differs); `FitWidth` / `FitHeight` for one bounded side, aspect ratio kept;
  `Fit` for a bounding box, never enlarging; `Fill` for the exact size, cropped around the center.
- **Output format** (`OutputFormat`): `Auto` (a lossy source without transparency becomes JPEG,
  anything else PNG); `Jpeg` (quality 1–100, default 75; transparency is refused unless a
  `background` composites it away, and output is capped at 65535 px); `Png` (always lossless);
  `WebP` (quality 0–100; with one it is lossy; capped at 16383 px).
- **Resampling kernel** (`ResizeFilter`): `Nearest`, `Triangle`, `CatmullRom`, `Gaussian`, and
  `Lanczos3` (the default).

## Decode once, encode several times

When one geometry becomes several encodings:

```rust
use tola_image::{DecodedSource, NO_CANCELLATION, OutputFormat};

let options = ResizeOptions {
    width: Some(640),
    operation: ResizeOperation::FitWidth,
    ..ResizeOptions::default()
};
let png = ImageRecipe::resolve(&metadata, ResizeOptions { format: OutputFormat::Png, ..options })?;
let webp = ImageRecipe::resolve(
    &metadata,
    ResizeOptions { format: OutputFormat::WebP, quality: Some(80), ..options },
)?;

let decoded = DecodedSource::open(&source, &NO_CANCELLATION)?;
let pixels = decoded.pixels(png.pixels(), &NO_CANCELLATION)?; // both recipes share this geometry
let (width, height) = (pixels.pixels().width(), pixels.pixels().height());
let png_bytes = pixels.encode(&png, &NO_CANCELLATION)?;
let webp_bytes = pixels.encode(&webp, &NO_CANCELLATION)?;
```

## Cancellation

Rendering polls whatever token it is given — any `Cancellation`, including a `Fn() -> bool`
closure; `&NO_CANCELLATION` when nothing can request a stop. The checks run around decoding,
resampling, and encoding; a stop returns `ImageCancelled`, apart from codec failures.

`IMAGE_PIPELINE_REVISION` is part of every variant's identity: a pipeline change regenerates all
of them.
