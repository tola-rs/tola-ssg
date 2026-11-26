#show: package(
  summary: [Generate an XML sitemap listing published site documents for search engines.],
  related: ("[site]", "@tola/feed", "@tola/schema#timestamp"),
)

#export(
  "sitemap",
  summary: [Emit a `sitemap.xml` file listing absolute URLs and optional `lastmod` timestamps.],
  params: (
    output: [Publish-relative output path, defaulting to `"sitemap.xml"`.],
    targets: [Array of unique complete documents: each entry is an output path, document label, location, or `(target:, lastmod:)` dictionary. Element targets or fragments are rejected.],
  ),
)[
  Requires `site.origin` in #ref("[site]"). `lastmod` accepts a Typst `datetime` (UTC), a `"YYYY-MM-DD"` date string, or an RFC 3339 timestamp string (pass `.rfc3339` from #ref("@tola/schema#timestamp")).

  #example(title: [Generate `sitemap.xml` with last-modified dates], check: "site")[
  ```typst
  #import "@tola/sitemap:0.0.0": sitemap

  #document("index.html")[Home]
  #document("guide/index.html")[Guide]

  #sitemap(targets: (
    "index.html",
    (target: "guide/index.html", lastmod: datetime(year: 2026, month: 9, day: 1)),
  ))
  ```
  ]
]
