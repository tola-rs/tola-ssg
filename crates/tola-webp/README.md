# tola-webp

Pure Rust lossy still-image WebP encoder with exact alpha; no C, no unsafe.

```rust
let webp = tola_webp::encode_rgba(&rgba, width, height, 75)?;
```

Straight-alpha RGBA8; dimensions `1..=16383`; buffer exactly `width * height * 4`; quality
`0..=100` (lossy at 100). `tola-image` selects it with `default-features = false,
features = ["pure-webp"]`; still images only.

```sh
cargo test --locked -p tola-webp
```

