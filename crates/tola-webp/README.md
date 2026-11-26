# tola-webp

Pure-Rust lossy still-image WebP encoder with exact alpha (no C, no `unsafe`).

```rust
let webp = tola_webp::encode_rgba(&rgba, width, height, 75)?;
```

- Input: straight-alpha RGBA8 (`width * height * 4` bytes), dimensions `1..=16383`, quality `0..=100`.
- `encode_rgba_to` writes to a caller buffer with optional cancellation and worker limit.
- Extends nearby colors into fully transparent pixels before chroma subsampling while keeping alpha exact.
