# tola-image

Decodes, resizes, and encodes images from in-memory bytes.

```rust
let metadata = tola_image::inspect(&source)?;
let recipe = ImageRecipe::resolve(&metadata, ResizeOptions {
    width: Some(640), operation: ResizeOperation::FitWidth, ..ResizeOptions::default()
})?;
let encoded = tola_image::render(&source, &recipe, &NO_CANCELLATION)?;
```

- Inputs: JPEG, PNG, WebP, GIF, BMP, and SVG (rasterized on explicit raster format; AVIF unsupported).
- Outputs: JPEG, PNG, and WebP (lossy via `tola-webp`, lossless RGBA).
- Supports `Scale`, `FitWidth`, `FitHeight`, `Fit`, and `Fill` with `Nearest`, `Triangle`, `CatmullRom`, `Gaussian`, and `Lanczos3` filters, plus `DecodedSource` reuse and cancellation.
