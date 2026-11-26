#show: package(
  summary: [Generate slugs and convert between site routes, publish output paths, and browser URLs.],
  related: ("[site]", "[assets]", "@tola/source"),
)

Keep the three layers distinct: a *route* (`/guide/`) is a site-root logical address without deployment prefix, an *output path* (`guide/index.html`) is a file path relative to `build.publish-dir` passed to `#document(...)`, and a *browser URL* (`/docs/guide/`) adds `site.base-path` and percent-encoding.

#export(
  "slugify",
  summary: [Turn text into a URL- and filename-safe slug normalized to Unicode NFC.],
  params: (
    text: [Source text to slugify. Fails if no usable characters remain.],
    mode: [`"unicode"` (default) keeps letters and numbers; `"ascii"` transliterates Han characters by pronunciation and strips accents.],
    case: [Letter casing: `"lower"` (default), `"upper"`, `"capitalize"`, or `"preserve"`.],
    separator: [Word separator: `"-"` (default) or `"_"`.],
    language: [BCP 47 tag selecting Han readings in `"ascii"` mode. Primary subtag `"ja"` (case-insensitive) uses Japanese readings; all other tags use Chinese readings.],
  ),
  related: ("join-route", "@tola/source#all-sources"),
)[
  Preserves letters, combining marks, digits, and URL-safe punctuation; other characters act as word separators. In `"ascii"` mode, Han characters are read in word context to resolve polyphonic characters.

  #example(title: [Generate Unicode and ASCII slugs], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": slugify

  #let english = slugify("Deep Notes")
  #let ascii = slugify("北京 Café", mode: "ascii")
  #let upper = slugify("Deep Notes", case: "upper", separator: "_")
  ```
  ]
]

#export(
  "join-route",
  summary: [Join path segments into a canonical site-root route starting and ending with `/`.],
  params: (
    segments: [Array of filename segments. Each segment must be non-empty, not `.` or `..`, and free of `/` and `\`.],
  ),
  related: ("slugify", "output-of", "@tola/source#all-sources"),
)[
  Segments are kept verbatim—call `slugify` first when you want lowercase or transliterated URLs. An empty array returns `"/"`.

  #example(title: [Build a document output path from route segments], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": join-route, output-of, slugify

  #let route = join-route(("posts", "Deep Notes").map(slugify))
  #document(output-of(route), title: [Deep Notes])[Article body]
  ```
  ]
]

#export(
  "output-of",
  summary: [Map a decoded site-root route to the publish-relative file path passed to `#document`.],
  params: (
    route: [Decoded site-root route without deployment prefix (for example `"/guide/"` or `"/manual.pdf"`).],
  ),
  related: ("route-of", "output-url", "decode-url-path"),
)[
  Routes ending in `/` resolve to `index.html` inside that directory (`"/"` → `"index.html"`, `"/guide/"` → `"guide/index.html"`). Other routes keep their filename (`"/manual.pdf"` → `"manual.pdf"`). Literal `%` and `#` are treated as ordinary filename characters; if the input is percent-encoded, call `decode-url-path` first.

  #example(title: [Convert routes to document output paths], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": output-of

  #let home = output-of("/")
  #let guide = output-of("/guide/")
  #let pdf = output-of("/manual.pdf")
  ```
  ]
]

#export(
  "route-of",
  summary: [Convert a publish-relative output path back to its canonical site-root route.],
  params: (
    output: [Decoded file path under `build.publish-dir`, without a leading `/`.],
  ),
  related: ("output-of", "route-url"),
)[
  Directory index files collapse to directory routes (`"index.html"` → `"/"`, `"guide/index.html"` → `"/guide/"`); other files keep their filename (`"404.html"` → `"/404.html"`). Result omits `base-path`, `origin`, and percent-encoding.
]

#export(
  "output-url",
  summary: [Convert a publish-relative output path into a percent-encoded browser URL with `site.base-path`.],
  params: (
    output: [Decoded file path under `build.publish-dir`, without a leading `/` or deployment prefix.],
    base-path: [Deployment mount path starting and ending with `/`. Defaults to `site.base-path` when `auto`.],
    origin: [Optional HTTP(S) origin (such as `"https://example.com"`) to produce an absolute URL. Omit or pass `none` for a root-relative URL.],
  ),
  related: ("route-url", "asset-url", "output-of"),
)[
  Directory index files (`guide/index.html`) become directory URLs (`/guide/`). Literal `%` in filenames is percent-encoded once to `%25`. Because `output-url` does not require the file to exist at call time, use it to link to outputs produced later by #ref("[[hooks.generate-outputs]]"); Tola checks HTML links after hooks finish.

  #example(title: [Link between pages with automatic `base-path`], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": output-url

  #let guide = "guide/index.html"
  #document("index.html", title: [Home])[
    #link(output-url(guide))[Read the guide]
  ]
  #document(guide, title: [Guide])[Start here.]
  ```
  ]
]

#export(
  "route-url",
  summary: [Convert a decoded site-root route into a browser URL with `site.base-path`.],
  params: (
    route: [Decoded site-root route without deployment prefix.],
    base-path: [Deployment mount path starting and ending with `/`. Defaults to `site.base-path` when `auto`.],
    origin: [Optional HTTP(S) origin to produce an absolute URL.],
  ),
  related: ("output-url", "output-of", "decode-url-path"),
)[
  Equivalent to `output-url(output-of(route), ...)`: both `"/guide/"` and `"/guide/index.html"` produce `"/guide/"` (prefixed with `base-path`). If the route is already percent-encoded, decode it once with `decode-url-path` first.
]

#export(
  "asset-url",
  summary: [Return the browser URL for a static asset declared under `[assets]`.],
  params: (
    declared-url: [Site-root URL path as declared in `[assets]` (leading `/`, without `site.base-path`). Literal `%` and `#` in filenames must be escaped as `%25` and `%23`.],
  ),
  related: ("[assets]", "output-url"),
)[
  - For `assets.files`, pass the entry's `url` (for example `"/styles/base.css"`).
  - For `assets.trees`, join the tree's `url-prefix` and the member's relative path (for example `"/assets/images/logo.svg"`).

  Fails if the asset is not enabled under #ref("[assets]"). Preserves exact filenames including `index.html`, prepends `site.base-path`, and appends `?h=<hash>` when `assets.cache-busting` is enabled (editor checks omit the hash suffix; run `tola check` to verify final URLs). For symlink rules, see #ref("[assets]"). For hook-generated files, use `output-url`; for resized images, use the `.url` returned by #ref("@tola/image#resize-image").

  #table(
    columns: 3,
    table.header([], [`output-url(output)`], [`asset-url(declared-url)`]),
    [Input], [Decoded publish-relative path (`"assets/logo.png"`).], [Site-root declared URL (`"/assets/logo.png"`).],
    [Validation], [Checked after build hooks finish.], [Must match an enabled `[assets]` entry immediately.],
    [Output], [With `base-path`; collapses `index.html` to `/`.], [With `base-path`; keeps `index.html`; adds `?h=<hash>` when cache-busting is on.],
  )

  #example(title: [Reference a declared static image], check: "site")[
  ```typst
  #import "@tola/address:0.0.0": asset-url

  #document("index.html")[
    #html.img(src: asset-url("/assets/logo.png"), alt: "Site logo")
  ]
  ```
  ]
]

#export(
  "decode-url-path",
  summary: [Percent-decode a URL path once into a site-root route suitable for `output-of` or `route-url`.],
  params: (
    url-path: [Site-root URL path without scheme, host, query, or fragment. Rejects encoded `/` or `\` and invalid filename segments.],
  ),
  related: ("output-of", "route-url", "@tola/schema#url-path"),
)[
  Decodes sequences such as `"/caf%C3%A9/"` → `"/café/"` and `"/100%25/"` → `"/100%/"`. To validate permalink fields inside source metadata with field-level diagnostics, prefer #ref("@tola/schema#url-path").
]
