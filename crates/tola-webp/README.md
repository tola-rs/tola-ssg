# tola-webp

Pure Rust lossy still-image WebP encoder with exact alpha. No C, no unsafe.

```rust
let webp = tola_webp::encode_rgba(&rgba, width, height, 75)?;
```

Takes straight-alpha RGBA8; dimensions `1..=16383`; the buffer must be exactly
`width * height * 4` bytes; quality `0..=100` (lossy at 100). `tola-image` selects it with
`default-features = false, features = ["pure-webp"]`. Still images only.

```sh
cargo test --locked -p tola-webp
```

Continuation: [HANDOFF.md](HANDOFF.md).
