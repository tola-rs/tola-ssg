#show: package(
  summary: [Read resolved options and published static assets for a configured Tola module.],
  related: ("[modules]", "[options]"),
)

#export(
  "get-module",
  summary: [Return the runtime handle (`name`, `options`, `assets`) of a configured module instance.],
  params: (
    name: [Module alias declared under `[modules.<alias>]` in `tola.toml`. May be omitted only when called from inside a `@tola-modules/*` package, where it returns the current module instance.],
  ),
)[
  Each alias in `tola.toml` is a distinct instance even when two aliases share the same module source.

  - `name`: module alias string.
  - `options`: module options dictionary after merging site overrides from `[modules.<alias>.options]`.
  - `assets.files`: Typst `path` values for reading module asset files.
  - `assets.urls`: published browser URLs (already prefixed with `site.base-path` and cache-busting `?h=<hash>` when enabled).

  #example(title: [Load a module's stylesheet and title option])[
  ```typst
  #import "@tola/module:0.0.0": get-module

  #let theme = get-module("theme")
  #html.link(rel: "stylesheet", href: theme.assets.urls.at("style.css"))
  = #theme.options.title
  ```
  ]
]
