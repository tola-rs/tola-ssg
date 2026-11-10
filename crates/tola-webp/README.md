# tola-webp

Pure Rust lossy still-image WebP encoder with exact alpha. No C, no unsafe.

```rust
let webp = tola_webp::encode_rgba(&rgba, width, height, 75)?;
```

Takes straight-alpha RGBA8; dimensions `1..=16383`; the buffer must be exactly
`width * height * 4` bytes; quality `0..=100` (lossy at 100). `tola-image` uses this crate for
lossy WebP. `encode_rgba_to` writes to a caller-supplied destination and accepts a cancellation
callback and a worker limit.

```sh
cargo test --locked -p tola-webp
```

