#show: config(
  summary: [Configure the site's public origin, mount path, title, authors, and languages.],
  related: ("@tola/site", "@tola/address"),
)

These fields populate `site` in #ref("@tola/site"). When `origin` is set, `site.url` joins `origin` and `base-path` (for example `"https://example.com/docs/"`); otherwise `site.url` is `none`.

Pass `--origin` or `--base-path` on the command line to override those values for a single run.

#example(title: [Deploy under a subpath with locale metadata])[
  ```toml
  [site]
  origin = "https://example.com"
  base-path = "/docs/"
  title = "Field Notes"
  language = "en-US"
  languages = ["en-US", "zh-Hans"]
  ```
]

#example(title: [Apply site language settings in Typst])[
  ```typst
  #import "@tola/site:0.0.0": site
  #set text(
    lang: site.language.lang,
    script: site.language.script,
    region: site.language.region,
  )
  ```
]

#key("origin")[
  Public scheme, host, and optional port, such as `"https://example.com"`. Put mount subpaths in `base-path`, not here. Required when generating feeds or sitemaps.
]

#key("base-path")[
  Subpath where the site is mounted under its origin, such as `"/docs/"`. Must start and end with `/`.
]

#key("title")[
  Site name exposed as `site.title` for templates, page titles, and feeds.
]

#key("authors")[
  List of site authors, each with a required `name` and optional `email` and `url`, for example `[{ name = "Alice", email = "alice@example.com" }]`.
]

#key("description")[
  Short site summary exposed as `site.description` for metadata and feeds.
]

#key("language")[
  Primary site language. Accepts either a BCP 47 tag (such as `"zh-Hant-TW"`) or a table `{ lang = "zh", script = "Hant", region = "TW" }`. Exposed as `site.language` with `tag`, `lang`, `script`, and `region`.
]

#key("languages")[
  Languages supported by the site (for example `["en", "zh-Hans"]`), using the same format as `language`. When non-empty, it must include `language`. Routing and language switchers are owned by your site templates.
]
