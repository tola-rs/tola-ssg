#show: package(
  summary: [Build HTML `<head>` metadata elements: title, description, canonical link, Open Graph, and Twitter cards.],
  related: ("@tola/site", "@tola/image#image-srcset"),
)

Each function returns an array of HTML elements; concatenate them with `+` and pass `.join()` into `html.head(...)`.

#export(
  "head-metadata",
  summary: [Return standard `<meta charset>`, viewport, `<title>`, and `<meta name="description">` elements.],
  params: (
    site: [Site dictionary providing default title and description (usually `site` from `@tola/site`).],
    title: [Page title. `auto` uses `site.title`; `none` or blank omits `<title>`.],
    description: [Page description. `auto` uses `site.description`; `none` or blank omits the description tag.],
    charset: [Character encoding, defaulting to `"utf-8"`. Pass `none` or blank to omit.],
    viewport: [Viewport content, defaulting to `"width=device-width, initial-scale=1"`. Pass `none` or blank to omit.],
  ),
  related: ("canonical", "open-graph", "twitter-card"),
)[
  #example(title: [Compose complete HTML `<head>` metadata], check: "default")[
  ```typst
  #import "@tola/head:0.0.0": canonical, head-metadata, open-graph, twitter-card
  #import "@tola/site:0.0.0": site

  #let url = "https://example.com/guide/"
  #let cover = (url: "https://example.com/cover.png", alt: "Guide cover")
  #let tags = (
    head-metadata(site, title: "Trail Guide", description: "Plan a day hike.")
    + canonical(url)
    + open-graph(title: "Trail Guide", url: url, images: (cover,))
    + twitter-card(card: "summary_large_image", title: "Trail Guide", image: cover)
  )

  #document("guide/index.html", title: "Trail Guide")[
    #html.html[
      #html.head(tags.join())
      #html.body[Trail Guide]
    ]
  ]
  ```
  ]
]

#export(
  "canonical",
  summary: [Return a `<link rel="canonical">` element for the page's canonical HTTP(S) URL.],
  params: (
    href: [Absolute `http://` or `https://` URL for the current page.],
  ),
  related: ("head-metadata", "open-graph"),
)

#export(
  "open-graph",
  summary: [Return Open Graph `<meta property="og:*">` elements for social link previews.],
  params: (
    title: [Non-empty page title.],
    kind: [Open Graph type such as `"website"` (default) or `"article"`.],
    url: [Absolute HTTP(S) page URL.],
    images: [`none` to omit image tags, or a non-empty array of `(url:, alt:)` image dictionaries.],
    description: [Optional summary written when non-empty.],
    site-name: [Optional site name written when non-empty.],
    locale: [Optional locale written when non-empty.],
  ),
  related: ("twitter-card", "head-metadata"),
)

#export(
  "twitter-card",
  summary: [Return Twitter Card `<meta name="twitter:*">` elements for link previews.],
  params: (
    card: [Card type: `"summary"` or `"summary_large_image"`.],
    title: [Non-empty card title.],
    image: [Optional `(url:, alt:)` dictionary; required when `card` is `"summary_large_image"`.],
    description: [Optional description written when non-empty.],
    handle: [Optional site account handle (`twitter:site`).],
    creator: [Optional author account handle (`twitter:creator`).],
  ),
  related: ("open-graph", "head-metadata"),
)
