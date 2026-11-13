// @tola/head:0.0.0 - HTML head entries and social cards

/// Return the charset, viewport, title, and description entries for `html.head`.
///
/// Use `+` to combine the arrays and `entries.join()` to pass their content to `html.head`.
/// The `#document` title remains available as document metadata.
///
/// Example: Write page metadata and link previews:
///
/// Replace `site.typ` in a site created with `tola init` with the code below. The example uses an absolute page
/// URL and a cover image URL. For a published site, use your actual page URL and a public image.
/// These functions write metadata. Publish the image separately, for example through `[assets]`.
///
/// ```typst
/// #import "@tola/site:0.0.0": site
/// #import "@tola/head:0.0.0": canonical, head-metadata, open-graph, twitter-card
///
/// #let url = "https://example.test/guide/"
/// #let cover = (url: "https://example.test/cover.png", alt: "A map of the trail")
/// #let entries = (
///   head-metadata(site, title: "Trail guide", description: "Plan a day hike.")
///   + canonical(url)
///   + open-graph(title: "Trail guide", url: url, images: (cover,))
///   + twitter-card(card: "summary_large_image", title: "Trail guide", image: cover)
/// )
///
/// #document("guide/index.html", title: "Trail guide")[
///   #html.html[
///     #html.head(entries.join())
///     #html.body[Trail guide]
///   ]
/// ]
/// ```
///
/// Run `tola build --minify=false` from the site root. With the default publish directory,
/// the `<head>` in `public/guide/index.html` contains these tags (shown with the outer indentation removed).
///
/// ```html
/// <head>
///   <meta charset="utf-8">
///   <meta name="viewport" content="width=device-width, initial-scale=1">
///   <title>Trail guide</title>
///   <meta name="description" content="Plan a day hike.">
///   <link rel="canonical" href="https://example.test/guide/">
///   <meta property="og:title" content="Trail guide">
///   <meta property="og:type" content="website">
///   <meta property="og:url" content="https://example.test/guide/">
///   <meta property="og:image" content="https://example.test/cover.png">
///   <meta property="og:image:alt" content="A map of the trail">
///   <meta name="twitter:card" content="summary_large_image">
///   <meta name="twitter:title" content="Trail guide">
///   <meta name="twitter:image" content="https://example.test/cover.png">
///   <meta name="twitter:image:alt" content="A map of the trail">
/// </head>
/// ```
///
/// Example: Omit the title and description entries:
///
/// ```typst
/// #import "@tola/site:0.0.0": site
/// #import "@tola/head:0.0.0": head-metadata
/// #let entries = head-metadata(site, title: none, description: none)
/// #assert.eq(entries.len(), 2)
/// ```
///
/// Related: canonical, open-graph, twitter-card
///
/// - site (dictionary): supplies the defaults, usually the dictionary `@tola/site` provides.
/// - title (auto | none | content | string): If omitted or `auto`, use the site's title. `none` or a blank
///   string omits the entry.
/// - description (auto | none | content | string): If omitted or `auto`, use the site's description. `none`
///   or a blank string omits the entry.
/// - charset (none | string): character encoding. Defaults to `utf-8`. Use `none` or a blank string to omit it.
/// - viewport (none | string): viewport content. Defaults to `width=device-width, initial-scale=1`.
///   Use `none` or a blank string to omit it.
/// -> array
#let head-metadata = {
  import "@tola/schema:0.0.0": literal, optional, parse, schema, union
  import "@tola/document:0.0.0": plain-text
  import "head-fields.typ": optional-text

  (
    site,
    title: auto,
    description: auto,
    charset: "utf-8",
    viewport: "width=device-width, initial-scale=1",
  ) => {
    let head-text = union(content, optional-text)
    let text-or-auto = union(literal(auto), head-text)
    let site-texts = schema(
      (
        title: optional(head-text, default: none),
        description: optional(head-text, default: none),
      ),
      unknown: "keep",
    )
    let options = parse(
      (
        site: site,
        title: title,
        description: description,
        charset: charset,
        viewport: viewport,
      ),
      schema((
        site: site-texts,
        title: text-or-auto,
        description: text-or-auto,
        charset: optional-text,
        viewport: optional-text,
      )),
    )
    let title = if options.title == auto { options.site.title } else { options.title }
    let description = if options.description == auto {
      options.site.description
    } else {
      options.description
    }

    let entries = ()
    if options.charset != none { entries.push(html.meta(charset: options.charset)) }
    if options.viewport != none {
      entries.push(html.meta(name: "viewport", content: options.viewport))
    }
    if title != none { entries.push(html.title(plain-text(title))) }
    if description != none {
      entries.push(html.meta(name: "description", content: plain-text(description)))
    }
    entries
  }
}

/// The canonical link entry for a page, as an array to join into a head.
///
/// A page declares one canonical URL, and the entry is the head's `<link rel="canonical">`.
///
///
/// ```typst
/// #import "@tola/head:0.0.0": canonical
/// #let entries = canonical("https://example.test/guide/")
/// #assert.eq(entries.len(), 1)
/// ```
///
/// Related: head-metadata
///
/// - href (string): the page's absolute URL, with an `http` or `https` scheme.
/// -> array
#let canonical = {
  import "@tola/schema:0.0.0": http-url, parse, schema

  href => {
    let href = parse((href: href), schema((href: http-url))).href
    (html.link(rel: "canonical", href: href),)
  }
}

/// The Open Graph entries for a page, as an array to join into a head.
///
/// `title` and `url` are required. `kind` defaults to `website`. Use `images: none` to omit
/// the image tags. A complete Open Graph object requires an image.
///
/// An optional text value writes an entry only when it is non-blank.
///
/// Example: Declare Open Graph entries with an image:
///
/// ```typst
/// #import "@tola/head:0.0.0": open-graph
/// #let entries = open-graph(
///   title: "Guide", kind: "website", url: "https://example.test/guide/",
///   images: ((url: "https://example.test/cover.png", alt: "Cover"),),
/// )
/// #assert.eq(entries.len(), 5)
/// ```
///
/// Example: Omit image tags:
///
/// ```typst
/// #import "@tola/head:0.0.0": open-graph
/// #let entries = open-graph(
///   title: "Guide", url: "https://example.test/guide/",
/// )
/// #assert.eq(entries.len(), 3)
/// ```
///
/// Related: head-metadata, twitter-card
///
/// - title (string): the page's title. It must contain text.
/// - kind (string): the page's Open Graph type, such as `website` or `article`. Defaults to
///   `website`. It must contain text.
/// - url (string): the page's absolute HTTP(S) URL.
/// - images (none | array): `none` to omit image tags, or a nonempty array of image
///   dictionaries. Each image requires an absolute HTTP(S) `url` and a non-blank `alt`.
///   Optional fields are `secure-url` (an absolute HTTPS URL), `media-type` (a string such as
///   `image/png`), and `width` and `height` (positive integer pixel counts). Optional fields
///   accept `none`; blank text omits the entry. Each image's entries stay together, in the order
///   you give the images.
/// - description (none | string): the page's description; written only when set.
/// - site-name (none | string): the site's name; written only when set.
/// - locale (none | string): the page's locale; written only when set.
/// -> array
#let open-graph = {
  import "@tola/schema:0.0.0": (
    array-of, http-url, https-url, map, non-empty, nullable, optional, parse, refine, schema, trim,
  )
  import "head-fields.typ": empty-to-none, optional-text

  (
    title: auto,
    kind: "website",
    url: auto,
    images: none,
    description: none,
    site-name: none,
    locale: none,
  ) => {
    let property-meta(property, value) = html.elem(
      "meta",
      attrs: (property: property, content: value),
    )
    let pixel-count = refine(int, value => value > 0, message: "must be a positive pixel count")
    let secure-url = map(nullable(str), empty-to-none, output: nullable(https-url))
    let og-image = schema((
      url: http-url,
      alt: non-empty(trim(str)),
      secure-url: optional(secure-url, default: none),
      media-type: optional(optional-text, default: none),
      width: optional(nullable(pixel-count), default: none),
      height: optional(nullable(pixel-count), default: none),
    ))
    let options = parse(
      (
        title: title,
        kind: kind,
        url: url,
        images: images,
        description: description,
        site-name: site-name,
        locale: locale,
      ),
      schema((
        title: non-empty(trim(str)),
        kind: non-empty(trim(str)),
        url: http-url,
        images: nullable(non-empty(array-of(og-image))),
        description: optional-text,
        site-name: optional-text,
        locale: optional-text,
      )),
    )

    let entries = (
      property-meta("og:title", options.title),
      property-meta("og:type", options.kind),
      property-meta("og:url", options.url),
    )
    for (property, value) in (
      ("og:description", options.description),
      ("og:site_name", options.site-name),
      ("og:locale", options.locale),
    ) {
      if value != none { entries.push(property-meta(property, value)) }
    }
    if options.images != none {
      for image in options.images {
        entries.push(property-meta("og:image", image.url))
        if image.secure-url != none {
          entries.push(property-meta("og:image:secure_url", image.secure-url))
        }
        if image.media-type != none {
          entries.push(property-meta("og:image:type", image.media-type))
        }
        if image.width != none {
          entries.push(property-meta("og:image:width", str(image.width)))
        }
        if image.height != none {
          entries.push(property-meta("og:image:height", str(image.height)))
        }
        entries.push(property-meta("og:image:alt", image.alt))
      }
    }
    entries
  }
}

/// The Twitter card entries for a page, as an array to join into a head.
///
/// `card` and `title` are always written, and a `summary_large_image` card also requires `image`.
/// An optional text value writes an entry only when it is non-blank.
///
///
/// ```typst
/// #import "@tola/head:0.0.0": twitter-card
/// #let entries = twitter-card(
///   card: "summary_large_image", title: "Guide",
///   image: (url: "https://example.test/cover.png", alt: "Cover"),
/// )
/// #assert.eq(entries.len(), 4)
/// ```
///
/// Related: head-metadata, open-graph
///
/// - card (string): the card type: `"summary"` or `"summary_large_image"`.
/// - title (string): the page's title. It must contain text.
/// - image (none | dictionary): required for `summary_large_image`. Pass an `(url:, alt:)` dictionary
///   with an absolute HTTP(S) URL and non-blank alt text.
/// - description (none | string): the page's description; written only when set.
/// - handle (none | string): the site's handle; written only when set.
/// - creator (none | string): the creator's handle; written only when set.
/// -> array
#let twitter-card = {
  import "@tola/schema:0.0.0": check, enum-of, http-url, issue, non-empty, nullable, optional, parse, schema, trim
  import "head-fields.typ": optional-text

  (
    card: auto,
    title: auto,
    image: none,
    description: none,
    handle: none,
    creator: none,
  ) => {
    let card-image = schema((
      url: http-url,
      alt: non-empty(trim(str)),
    ))
    let options = parse(
      (
        card: card,
        title: title,
        image: image,
        description: description,
        handle: handle,
        creator: creator,
      ),
      check(
        schema((
          card: enum-of(("summary", "summary_large_image")),
          title: non-empty(trim(str)),
          image: nullable(card-image),
          description: optional-text,
          handle: optional-text,
          creator: optional-text,
        )),
        options => if options.card == "summary_large_image" and options.image == none {
          (issue("is required for a `summary_large_image` card", path: ("image",)),)
        } else {
          ()
        },
        on: ("card", "image"),
      ),
    )

    let entries = (
      html.meta(name: "twitter:card", content: options.card),
      html.meta(name: "twitter:title", content: options.title),
    )
    for (name, value) in (
      ("twitter:description", options.description),
      ("twitter:site", options.handle),
      ("twitter:creator", options.creator),
    ) {
      if value != none { entries.push(html.meta(name: name, content: value)) }
    }
    if options.image != none {
      entries.push(html.meta(name: "twitter:image", content: options.image.url))
      entries.push(html.meta(name: "twitter:image:alt", content: options.image.alt))
    }
    entries
  }
}
