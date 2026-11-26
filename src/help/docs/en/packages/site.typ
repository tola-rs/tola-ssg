#show: package(
  summary: [Read resolved `[site]` settings, `[options]`, `build-mode`, and site-rooted paths inside Typst.],
  related: ("[site]", "[options]", "@tola/head"),
)

#export(
  "site",
  summary: [Dictionary of resolved `[site]` configuration, `[options]`, and the current build mode.],
  related: ("project-path", "[site]", "@tola/head#head-metadata"),
)[
  #table(
    columns: 2,
    table.header([Field], [Value]),
    [`origin`], [Configured origin string, or `none`.],
    [`base-path`], [Deployment prefix starting and ending with `/` (default `"/"`).],
    [`url`], [`origin + base-path`, or `none` when `origin` is unset.],
    [`title`, `description`], [Configured strings (empty string when unset).],
    [`authors`], [Array of `(name:, email:, url:)` dictionaries.],
    [`language`], [Default language record `(tag:, lang:, script:, region:)` (defaults to `"en"`; missing `script` is `auto`, missing `region` is `none`).],
    [`languages`], [Configured language list in order (defaults to `()`).],
    [`options`], [Dictionary from `[options]` in `tola.toml` (defaults to `(:)`).],
    [`build-mode`], [`"dev"` under `tola dev`, `"prod"` under `tola build`/`check`/`preview`, or `none` outside a build (editor, `tola query`, `tola inspect`).],
  )

  #example(title: [Skip heavy work during `tola dev`], check: "default")[
  ```typst
  #import "@tola/site:0.0.0": site

  #let is-dev = site.build-mode == "dev"
  #document("index.html")[#site.title]
  ```
  ]
]

#export(
  "project-path",
  summary: [Resolve a site-root-relative path as a native Typst `path`, even when called from a module or package.],
  params: (
    path: [Path relative to the site root directory (for example `"data/catalog.json"`).],
  ),
  related: ("site", "@tola/module#get-module"),
)[
  Pass the returned `path` directly to `read(...)`, `json(...)`, or `csv(...)` so themes and helper modules can load files from the consuming site.
]
