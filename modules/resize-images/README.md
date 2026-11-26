# Resize images

Resize JPEG, PNG, and WebP images declared in `[assets]`.

```toml
[assets]
trees = [{ source = "photos", url-prefix = "/gallery" }]

[modules.resize-images]
builtin = "resize-images"

[modules.resize-images.options]
include = ["photos/**"]
exclude = ["photos/drafts/**"]
widths = [480, 960]

[modules.resize-images.options.overrides."photos/cover.jpg"]
widths = [1200]
```

Link generated variants via `output-url("images/<alias>/<source>/w<width>.<format>")` from `@tola/address` (e.g. `images/resize-images/photos/cover.jpg/w1200.webp`). Sizes wider than the original are skipped unless `upscale = true`.

## Options

| Option | Default | Meaning |
| --- | --- | --- |
| `include`, `exclude` | `[]` | Source path patterns (site-relative, or module-relative with `source = { module = "ALIAS" }`) |
| `widths` | `[480, 960]` | Target widths in pixels |
| `format` | `"webp"` | `"webp"`, `"png"`, or `"jpg"` |
| `quality` | unset | WebP `0..=100` (unset = lossless), JPEG `1..=100` (unset = 75) |
| `upscale` | `false` | Allow enlarging smaller images |
| `overrides` | `{}` | Per-path overrides for `widths`, `format`, `quality`, or `upscale` |
| `jobs` | `1` | Max images processed concurrently |
| `dev` | `"reuse"` | `"reuse"`, `"run"`, or `"skip"` in `tola dev` |

Each encoded size is cached separately. To customize the module, copy it into your site and switch `builtin` to `path`.
