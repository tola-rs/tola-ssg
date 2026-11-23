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

Link the resized cover with `output-url("images/resize-images/photos/cover.jpg/w1200.webp")`
from `@tola/address`. The directory under `images/` is the module alias. Sizes
wider than the original are skipped unless `upscale = true`.

## Options

- `include` (empty by default) and `exclude` match site-relative source paths.
  For module assets, set `source = { module = "notes" }` and use paths relative
  to that module. Replacements use their site paths.
- `widths` defaults to `[480, 960]` pixels. `format` defaults to `webp` and also
  accepts `png` and `jpg`.
- `quality` accepts 0–100 for WebP and 1–100 for JPEG. Omit it for lossless WebP
  or JPEG quality 75.
- `overrides` sets `widths`, `format`, `quality`, or `upscale` for an exact source
  path. `jobs` limits concurrent source images and defaults to `1`.
- `dev` defaults to `"reuse"`, which skips the hook when inputs match. Use `"run"`
  on every rebuild or `"skip"` to omit resized images in development.

Each encoded size is cached separately. Images created in `before-build` can
enter through `[assets]`. Read the resized files in `after-publish`.

To edit the module, copy it to your site's `modules/resize-images/` and replace
`builtin` with `path = "modules/resize-images"`.
