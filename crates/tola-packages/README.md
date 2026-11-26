# tola-packages

Built-in `@tola/*:0.0.0` Typst packages shipped with Tola:

`site`, `module`, `address`, `icon`, `image`, `source`, `collection`, `datalog`, `schema`, `document`, `code`, `head`, `feed`, `sitemap`, `hash`, `uuid`, and `diagnostic` (`@tola/host` is internal).

- `builtin_packages()` lists public packages; `resolvable_packages()` includes `@tola/host` (used by the compiler and `tola editor packages`).
- Usage docs live in `tola skill` and `tola help "@tola/<name>" [export ...]`.
- `@tola/code` bundles code themes (`code-themes`) and extra syntaxes (`code-syntaxes`). Third-party theme sources and licenses are recorded in [`licenses/README.md`](licenses/README.md) (distributed as `PROVENANCE-TOLA-PACKAGES.md`).
