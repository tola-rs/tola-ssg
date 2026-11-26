#show: package(
  summary: [Generate RSS, Atom, or JSON Feed outputs from site documents.],
  related: ("[site]", "@tola/sitemap", "@tola/schema#timestamp"),
)

#export(
  "feed",
  summary: [Emit an RSS, Atom, or JSON Feed file into the site bundle.],
  params: (
    output: [Publish-relative output path. `auto` selects `feed.xml` (RSS), `atom.xml` (Atom), or `feed.json` (JSON Feed).],
    format: [Feed format: `"rss"` (default), `"atom"`, or `"json"`.],
    id: [Feed identifier. Required to be an absolute URI for Atom; `auto` derives it from `site.url`.],
    title: [Feed title (must be non-blank). `auto` uses `site.title`.],
    description: [Feed description. `auto` uses `site.description`.],
    language: [BCP 47 language tag. `auto` uses `site.language.tag`.],
    authors: [Array of `(name:, email:, url:)` author dictionaries. `auto` uses `site.authors`. Atom requires an author on the feed or on every entry.],
    entries: [Array of entry dictionaries in publication order. Defaults to `()`.],
  ),
)[
  Requires `site.origin` in #ref("[site]") so entry links resolve to absolute URLs.

  = Entry fields

  Each entry requires `target` (an output path, `(output:, fragment:)`, label, or location) and `published` (a Typst `datetime` interpreted in UTC, or an RFC 3339 timestamp string—pass `.rfc3339` from #ref("@tola/schema#timestamp")). Optional fields: `id` (keep stable across URL moves; must be an absolute URI in Atom), `title` (defaults to the target document title), `updated`, `summary`, `content`, and `authors`.

  #table(
    columns: 3,
    table.header([Format], [`summary`], [`content`]),
    [RSS], [`<description>`], [`<content:encoded>`],
    [Atom], [`<summary>`], [`<content>` (text or HTML)],
    [JSON Feed], [`summary` (plain text)], [`content_text` or `content_html`],
  )

  = Entry `content` forms

  - Plain string: written as plain text.
  - Simple Typst content (text, line breaks, emphasis, strong, strike, URL links): converted directly to HTML without layout or show rules.
  - Rendered HTML extraction: pass `(document: output, id: "post-body")` to extract that element (and its descendants, plus `<head>` styles) from the exported HTML, or omit `id` to take the entire `<body>`. Relative URL attributes are rewritten to absolute URLs.

  #example(title: [Publish an RSS feed with extracted HTML content], check: "site")[
  ```typst
  #import "@tola/feed:0.0.0": feed

  #let output = "notes/index.html"
  #document(output, title: [First Note])[
    #html.html[
      #html.head(html.title("First Note"))
      #html.body[
        #html.article(id: "post-body")[
          = First Note
          Full article body included in the feed.
        ]
      ]
    ]
  ]

  #feed(
    title: "Field Notes",
    entries: ((
      id: "urn:notes:first",
      target: output,
      published: datetime(year: 2026, month: 9, day: 1),
      summary: "Short summary for feed previews.",
      content: (document: output, id: "post-body"),
    ),),
  )
  ```
  ]
]
