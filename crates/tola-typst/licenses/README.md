# Fonts

`tola-typst` carries these fonts when its `embed-fonts` feature is enabled. They are
redistributed unmodified from [typst-assets](https://github.com/typst/typst-assets)
0.15.1, the release the workspace locks with checksum `bcee505dac6702dd1c5e65aa2e94a6179d19ee09e2e5637d7313db91765dc4e0`. The crate's
`NOTICE` is that release's notice.

The fonts are carried compressed in `assets/embedded-fonts.bin`, behind an index naming each
one; a build reads the file and never fetches it. Regenerate with `just scripts::fonts`,
which also fails when the carried fonts stop matching this release.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| LibertinusSerif-Regular.otf | 337132 | fcf06307a77367394fcb0ccb241e59eea70dba3d732be309647611224679c733 |
| LibertinusSerif-Bold.otf | 293908 | 0264914210ed51b3231ebc92ce529e9f2e166ba9eebf0cd4a579558690a27b64 |
| LibertinusSerif-Italic.otf | 332312 | 9a393d63d6e05f620d3dc0190dfd35a8ede58c0808cf0fc9de7fcb9c723e4c24 |
| LibertinusSerif-BoldItalic.otf | 259176 | 47a665259f09f554f5d133d7718cdad43ff462c6a6b2328f38023465e62d57ce |
| LibertinusSerif-Semibold.otf | 291640 | a4b3f28e85881db34695c1f005e4c79233a6caf3a2bd286c9b418c025fb99308 |
| LibertinusSerif-SemiboldItalic.otf | 343148 | 397f0d7aba35ae6a988948ae046c14c6b1e0d270fcc6e292b8bc29fd625c6101 |
| NewCMMath-Bold.otf | 1232148 | c6c0e060da57d4f44274705afb956013047231dc62be5a4b02a351ab0dc43f2f |
| NewCMMath-Book.otf | 1432068 | 2ea09ebc9167b1e1a66f31390dc917f2d4004ecfca72d51b28010e4ad6becd95 |
| NewCMMath-Regular.otf | 1306268 | d66ac1cc91c55c24d3636ae2df1238076debdff51841f9893fc5419cc2df3df7 |
| NewCM10-Regular.otf | 717588 | 328698d764ccdf7acf6bc1088aefd83a237f6a1d8b812de1e77ac5e4483bf3d1 |
| NewCM10-Bold.otf | 629288 | 40b0b32b63655fe802679ee4a14adb29c762a4712095d9c84cf7df7852b5152e |
| NewCM10-Italic.otf | 721176 | a55d07607eec48889b6e7e7d51d7703c3beb915a39690f34fe01b4e2a744a9d6 |
| NewCM10-BoldItalic.otf | 609000 | fb53f43500c8ffbcc6dfdea5a519961d475543174ea2a0dcbe16c08510299fdf |
| DejaVuSansMono-Bold.ttf | 331992 | bce60f1b4421acd9ea51ba6623d7024ecbe6817a953e3654df62a5e6bdf8f769 |
| DejaVuSansMono-BoldOblique.ttf | 253580 | 91713a71d550bba22c2a6b2bb2a9ad8f9a159e12e4e9f0a5b2677998ba21213e |
| DejaVuSansMono-Oblique.ttf | 251932 | 742097840c541870e8d6dc5c9b37bb1ceeea6c0dedd1d475faf903ef9df734b0 |
| DejaVuSansMono.ttf | 340712 | b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582 |
