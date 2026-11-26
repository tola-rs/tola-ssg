#show: config(
  summary: [Declare local SVG or Iconify icon collections used by `@tola/icon`.],
  related: ("@tola/icon", "[vendor]"),
)

Each table under `[icons.collections.<namespace>]` registers a collection whose table name becomes the `namespace` prefix in `icon("namespace:name")` (icon names are case-sensitive).

= Collection source types

#table(
  columns: 2,
  table.header([`source-type`], [Configuration]),
  [`"remote-json"`], [Iconify preset (`preset = "lucide"`, optional `version` and `mirrors`) or direct `url`. Run `tola update <namespace>` to download and pin in `tola.lock`; `tola vendor` copies locked JSON into `vendor/icons/`.],
  [`"local-svg-dir"`], [Site-relative directory of standalone SVG files (`path = "icons/brand"`). Each SVG must declare positive `viewBox` dimensions or absolute width/height, with text converted to paths.],
  [`"local-json"`], [Site-relative local IconifyJSON file (`path = "icons/custom.json"`).],
)

Run `tola inspect icons <namespace>` (or add `--interactive`) to browse available icon names. Modules may declare their own `[icons.collections]` in `tola-module.toml`; a site can override a module collection under `[modules.<alias>.icons.collections.<namespace>]` and update remote module collections with `tola update modules/<alias>/<namespace>`.

#example(title: [Register a remote Iconify preset and a local SVG directory])[
  ```toml
  [icons.collections.lucide]
  source-type = "remote-json"
  preset = "lucide"

  [icons.collections.brand]
  source-type = "local-svg-dir"
  path = "icons/brand"
  ```
]

#key("collections")[
  Map of `<namespace>` to icon collection source definitions (`remote-json`, `local-svg-dir`, or `local-json`).
]
