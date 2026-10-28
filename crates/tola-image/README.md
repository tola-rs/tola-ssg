# tola-image

Deterministic image processing: decoding, colour management, resampling, and encoding. Bytes
in, encoded variants out.

```rust
let metadata = tola_image::inspect(&source)?;
let recipe = ImageRecipe::resolve(&metadata, ResizeOptions {
    width: Some(640), operation: ResizeOperation::FitWidth, ..ResizeOptions::default()
})?;
let encoded = tola_image::render(&source, &recipe, &NO_CANCELLATION)?;
```

Reads JPEG, PNG, WebP, GIF, BMP, and SVG (measured, not resized); AVIF is unsupported.
Operations: `Scale`, `FitWidth`, `FitHeight`, `Fit`, `Fill`. Filters: `Nearest`, `Triangle`,
`CatmullRom`, `Gaussian`, `Lanczos3`. Formats: `Auto`, `Jpeg`, `Png`, `WebP`. `DecodedSource`
decodes once for several recipes; `Cancellation` is polled around every stage. The `pure-webp`
feature selects the Rust encoder instead of `libwebp`.
