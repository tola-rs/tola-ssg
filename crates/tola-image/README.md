# tola-image

Deterministic image processing: bytes in, encoded variant bytes out.

```rust
let metadata = tola_image::inspect(&source)?;
let recipe = ImageRecipe::resolve(&metadata, ResizeOptions {
    width: Some(640), operation: ResizeOperation::FitWidth, ..ResizeOptions::default()
})?;
let encoded = tola_image::render(&source, &recipe, &NO_CANCELLATION)?;
```

JPEG, PNG, WebP, GIF, BMP, SVG (measured, not resized); AVIF unsupported. Operations: `Scale`,
`FitWidth`, `FitHeight`, `Fit`, `Fill`. Filters: `Nearest`, `Triangle`, `CatmullRom`, `Gaussian`,
`Lanczos3`. Formats: `Auto`, `Jpeg`, `Png`, `WebP`. `DecodedSource` decodes once for several
recipes; `Cancellation` is polled around every stage. The `pure-webp` feature selects the Rust
encoder (`libwebp` otherwise).
