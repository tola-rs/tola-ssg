#show: config(
  summary: [Define arbitrary site-wide settings shared by Typst templates, Nu scripts, and hook toggles.],
  related: ("@tola/site", "[hooks]", "[modules]"),
)

Keys under `[options]` are defined by your site. Values are exposed to Typst via `site.options` (from #ref("@tola/site")), to Nu scripts via `use tola get-site; get-site | get options`, and to site hooks via `enabled-by`.

- TOML dates and timestamps convert to strings; numbers must be finite.
- Site hook `enabled-by = "banner.enabled"` reads a boolean at that dotted path under `[options]`. Module hooks read from their own `[modules.<alias>.options]` instead.
- When merged via #ref("include") or module overrides, nested tables merge key by key while arrays and scalars replace earlier values.

#example(title: [Share a feature flag between Typst and a build hook])[
  ```toml
  [options.banner]
  enabled = true
  text = "Preview release"

  [[hooks.generate-outputs]]
  name = "banner"
  nu = "tools/write-banner.nu"
  enabled-by = "banner.enabled"
  outputs = [{ file = "banner.txt" }]
  ```
]
