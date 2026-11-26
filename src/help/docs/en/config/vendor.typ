#show: config(
  summary: [Store local vendored copies of modules, Typst packages, and remote icons for offline and `--pure` builds.],
  related: ("[modules]", "[icons]", "[typst.fonts]"),
)

Run `tola vendor` to populate the vendor directory, then verify hermetic builds with `tola build --pure`. Commit both the vendor directory and `tola.lock` with the site.

Before replacing existing files, `tola vendor` validates prepared copies for three subdirectories:
- `modules/`: locked remote modules indexed by alias.
- `typst-packages/`: Typst packages organized by namespace, name, and version.
- `icons/`: one JSON file per remote icon collection.

Compiler fonts are not copied by `tola vendor`; keep them in the repository under #ref("[typst.fonts]"). Use `tola vendor --dry-run` to validate without writing, or `tola vendor --refresh` to re-fetch from configured sources.

- `--offline` disables network requests.
- `--pure` additionally rejects global package caches, system fonts, and any path or symlink target outside the site (bundled packages and fonts remain available). External tools invoked by hooks keep their own environment permissions.

#example(title: [Enable vendoring under `vendor/`])[
  ```toml
  [vendor]
  path = "vendor"
  ```
]

#key("path")[
  Site-relative directory for vendored copies. Must stay inside the site root and outside `.tola`.
]
