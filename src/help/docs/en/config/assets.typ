#show: config(
  summary: [Publish static files and directories such as images, stylesheets, fonts, and downloads.],
  related: ("@tola/address#asset-url", "[build.minify]", "include"),
)

Every enabled asset is published even if no page references it. Reference declared assets from Typst via #ref("@tola/address#asset-url") so `site.base-path` and optional cache-busting hashes are applied automatically.

= Selection and precedence

- `trees` publishes a directory under `url-prefix`, preserving relative paths. Filter entries with case-sensitive `include` and `exclude` patterns (relative to the site root, or module root in modules). An empty `include = []` selects nothing.
- When a file matches both `files` and `trees`, the `files` entry wins (even if disabled). Tree source directories may not nest, and URL prefixes may not duplicate or overlap.
- Each entry supports `enable = false`, `enabled-by = "option.path"`, and `minify = false` to preserve exact source bytes.
- When splitting configuration with #ref("include"), `files` and `trees` append across files; every published output path must remain unique.

= File symlinks

Symlinks inside `trees` are rejected by default. Set `follow-file-symlinks = true` on a tree entry to publish symlinks that target readable regular files under the symlink's own name (`include` and `exclude` match the symlink path). Targets may live outside the tree's `source` directory (but inside the site under `--pure`, inside the module for module assets, and always outside `build.content-dir` and `build.publish-dir`). To publish a symlinked directory, declare a separate `trees` entry for its target directory.

#example(title: [Publish a single file and a static directory])[
  ```toml
  [assets]
  cache-busting = true
  files = [{ source = "static/logo.png", url = "/assets/logo.png" }]
  trees = [
    { source = "static/icons", url-prefix = "/icons", include = ["static/icons/**/*.svg"] },
  ]
  ```
]

#key("cache-busting")[
  Append `?h=<hash>` to URLs returned by `asset-url` while keeping published filenames unchanged.
]

#key("trees")[
  Directory mappings (`{ source = "...", url-prefix = "/...", include = [...], exclude = [...], follow-file-symlinks = false }`).
]

#key("files")[
  Single-file mappings (`{ source = "...", url = "/..." }`). Takes precedence over `trees` for the same source file.
]
