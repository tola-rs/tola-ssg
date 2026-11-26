#show: config(
  summary: [Choose whether broken internal links, missing assets, and unknown fragments fail the build or warn.],
)

Tola validates internal references across HTML outputs before publishing. Absolute URLs (including those matching `site.origin`) are treated as external and skipped.

#example(title: [Downgrade missing internal page links to warnings])[
  ```toml
  [build.references]
  navigation = "warn"
  ```
]

#key("navigation")[
  Severity when an internal link points to a route that does not exist.
]

#key("resources")[
  Severity when a referenced resource (such as an image, stylesheet, or script) is missing or has an incompatible media type.
]

#key("fragments")[
  Severity when the target page exists but the `#fragment` ID does not appear in its HTML.
]
