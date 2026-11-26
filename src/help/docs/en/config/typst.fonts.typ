#show: config(
  summary: [Configure font search directories for the Typst compiler and SVG rasterization.],
  related: ("[assets]", "@tola-scripts/image#resize"),
)

These settings apply to Typst compilation (PDF export, SVG frames) and SVG text rasterization in site hooks or #ref("@tola-scripts/image#resize"). Web fonts loaded by browsers belong in #ref("[assets]") with CSS `@font-face`.

Tola searches `paths` first, then module fonts in the order listed in `modules`. When two fonts match equally well, the earlier source wins. Run `tola inspect fonts` (or `tola inspect fonts --json`) to check resolved family names and origins.

- `--pure` still allows Tola's embedded fonts, but rejects `system = true` and `--font-path`.
- Pass `--font-path DIR` to prepend a search directory for one run, or `--ignore-system-fonts` to skip system fonts.
- `tola vendor` does not copy compiler fonts; keep project fonts inside the repository under `paths`.

#example(title: [Use project and module fonts without system fonts])[
  ```toml
  [typst.fonts]
  paths = ["static/typst-fonts"]
  modules = [
    "theme",
    { module = "cjk", rename = { "IBM Plex Sans" = "CJK Plex Sans" } },
  ]
  system = false
  ```
]

#key("paths")[
  Site-relative directories scanned recursively for compiler fonts.
]

#key("modules")[
  Module aliases whose `[typst] fonts` directories are loaded after `paths`. Pass a string alias or `{ module = "alias", rename = { "Old Family" = "New Family" } }` (case-insensitive match) to register all weights and styles of a family under a distinct name.
]

#key("system")[
  Include operating-system fonts during compilation. Rejected under `--pure`.
]
