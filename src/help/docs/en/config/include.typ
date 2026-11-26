#show: config(
  summary: [Split site configuration across multiple local TOML files.],
  related: ("[[hooks.before-build]]", "[assets]", "[modules]"),
)

Place `include` at the top of `tola.toml` before the first `[table]` header. Each entry is either a path string or a table `{ path = "...", optional = true }`.

= Load order and paths

- Relative paths in `include` resolve from the directory of the file that declares them. Site-relative settings inside included files (such as `build.publish-dir` or a local module's `path`) still resolve from the site root.
- Entries must be literal local file paths. Globs, environment variables, and remote URLs are not expanded. Absolute paths follow the active input scope (for example, `--pure` requires every configuration file to stay inside the site).
- Missing files fail unless marked `optional = true`. Present files are always validated regardless of `optional`.
- Files apply in array order, depth-first: each file's own `include` list runs before the file itself, and `tola.toml` applies last before defaults fill remaining keys.
- Including the same file more than once applies it each time (which also repeats hooks and assets and can trigger duplicate names or output paths). Cycles are rejected.

= Merge rules

Tables merge key by key. Later scalars and ordinary arrays replace earlier values. Asset and hook arrays (`assets.files`, `assets.trees`, `hooks.before-build`, `hooks.generate-outputs`, `hooks.after-publish`) append in load order.

#table(
  columns: 2,
  table.header([Written later], [Result]),
  [Omit a key], [Keep the earlier value.],
  [`title = ""`], [Replace with an empty string (when allowed by that field).],
  [`languages = []` under `[site]`], [Replace earlier entries with an empty list.],
  [`files = []` under `[assets]`], [Append zero entries; earlier declarations remain.],
  [An empty table], [Keep all keys already set in that table.],
)

Use `tola inspect config --json --pretty` to inspect `config_sources` (load order and optional-missing files) and `field_origins` (which file set or overrode each key).

#example(title: [Share a base file with optional local overrides])[
  ```toml
  include = [
    "config/base.toml",
    { path = "config/local.toml", optional = true },
  ]
  ```
]
