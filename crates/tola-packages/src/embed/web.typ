// @tola/web:0.0.0 - plain text, head entries, math as SVG, social cards, feeds, and sitemaps

/// Read the text of a string or of your content.
///
/// Text characters, symbol characters, spaces, raw code, and smart quotes contribute their own
/// text. Every other element contributes the text of its content, so containers, formatting,
/// links, and mathematics are transparent. Metadata contributes nothing. The text comes from what
/// you wrote: no show rule runs, and nothing is laid out.
///
/// Reading a string or content always succeeds. An element that carries neither text nor content
/// of its own contributes nothing, and a warning names it where you wrote it.
///
/// Example - read text from a string and from content:
///
/// ```typst
/// #import "@tola/web:0.0.0": plain-text
/// #assert.eq(plain-text("Guide"), "Guide")
/// #assert.eq(plain-text([Hello, *world*!]), "Hello, world!")
/// ```
///
/// Example - an element that carries no text contributes nothing:
///
/// ```typst
/// #import "@tola/web:0.0.0": plain-text
/// #let image = html.img(src: "/cover.png", alt: "Cover")
/// #assert.eq(plain-text(image), "")
/// ```
///
/// Related: head-metadata, open-graph, twitter-card
#import "@tola/host:0.0.0": plain-text

/// The head entries for a document, as an array to join into a head.
///
/// The entries are the character set, the viewport, the title, and the description. Your template
/// writes the head: join these entries into `html.head(...)`, and keep the document's own metadata
/// separate. `+` joins arrays and keeps their entries flat and in order:
///
/// ```typst
/// #import "@tola/site:0.0.0": site
/// #import "@tola/web:0.0.0": canonical, head-metadata
/// #let entries = head-metadata(site, title: "Guide") + canonical("https://example.test/guide/")
/// #document("guide/index.html")[
///   #html.html[#html.head(entries.join()) #html.body[Guide]]
/// ]
/// ```
///
/// Example - omit the title and description entries:
///
/// ```typst
/// #import "@tola/site:0.0.0": site
/// #import "@tola/web:0.0.0": head-metadata
/// #let entries = head-metadata(site, title: none, description: none)
/// #assert.eq(entries.len(), 2)
/// ```
///
/// Related: canonical, open-graph, twitter-card
///
/// - site (dictionary): supplies the defaults, usually the dictionary `@tola/site` provides.
/// - title (auto | none | content | string): `auto` inherits the site's title; `none` or a blank
///   string omits the entry.
/// - description (auto | none | content | string): `auto` inherits the site's description; `none`
///   or a blank string omits the entry.
/// - charset (none | string): the document's character encoding; written unless you pass `none`.
/// - viewport (none | string): the viewport meta content; written unless you pass `none`.
/// -> array
#let head-metadata = {
  import "@tola/schema:0.0.0": literal, optional, parse, schema, union
  import "web-fields.typ": optional-text

  (
    site,
    title: auto,
    description: auto,
    charset: "utf-8",
    viewport: "width=device-width, initial-scale=1",
  ) => {
    let head-text = union(content, optional-text)
    let text-or-auto = union(literal(auto), head-text)
    let site-texts = schema((
      title: optional(head-text, default: none),
      description: optional(head-text, default: none),
    ), unknown: "keep")
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
/// Example - declare a page's canonical URL:
///
/// ```typst
/// #import "@tola/web:0.0.0": canonical
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

  (href) => {
    let href = parse((href: href), schema((href: http-url))).href
    (html.link(rel: "canonical", href: href),)
  }
}

/// The Open Graph entries for a page, as an array to join into a head.
///
/// `title`, `kind`, `url`, and `images` are required, and `images` holds at least one
/// `(url:, alt:, …)` dictionary.
///
/// An optional text value writes an entry only when it is non-blank.
///
/// Example - declare a page's Open Graph entries:
///
/// ```typst
/// #import "@tola/web:0.0.0": open-graph
/// #let entries = open-graph(
///   title: "Guide", kind: "website", url: "https://example.test/guide/",
///   images: ((url: "https://example.test/cover.png", alt: "Cover"),),
/// )
/// #assert.eq(entries.len(), 5)
/// ```
///
/// Related: head-metadata, twitter-card
///
/// - title (string): the page's title.
/// - kind (string): the page's Open Graph type, such as `website` or `article`.
/// - url (string): the page's absolute URL.
/// - images (array): a nonempty array of image dictionaries. Each requires an absolute HTTP(S)
///   `url` and a non-blank `alt`. Optional fields are `secure-url` (an absolute HTTPS URL),
///   `media-type` (a string such as `image/png`), and `width` and `height` (positive integer pixel
///   counts). Optional fields accept `none`; blank text omits the entry. Each image's entries stay
///   together, in the order you give the images.
/// - description (none | string): the page's description; written only when set.
/// - site-name (none | string): the site's name; written only when set.
/// - locale (none | string): the page's locale; written only when set.
/// -> array
#let open-graph = {
  import "@tola/schema:0.0.0": array-of, http-url, https-url, map, non-empty, nullable, optional, parse, refine, schema, trim
  import "web-fields.typ": empty-to-none, optional-text

  (
    title: auto,
    kind: auto,
    url: auto,
    images: auto,
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
        images: non-empty(array-of(og-image)),
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
    entries
  }
}

/// The Twitter card entries for a page, as an array to join into a head.
///
/// `card` and `title` are always written, and a `summary_large_image` card also requires `image`.
/// An optional text value writes an entry only when it is non-blank.
///
/// Example - declare a large-image card:
///
/// ```typst
/// #import "@tola/web:0.0.0": twitter-card
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
/// - title (string): the page's title.
/// - image (none | dictionary): required by a large-image card: an `(url:, alt:)` dictionary.
/// - description (none | string): the page's description; written only when set.
/// - handle (none | string): the site's handle; written only when set.
/// - creator (none | string): the creator's handle; written only when set.
/// -> array
#let twitter-card = {
  import "@tola/schema:0.0.0": check, enum-of, http-url, issue, non-empty, nullable, optional, parse, schema, trim
  import "web-fields.typ": optional-text

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

/// Render an equation as SVG during HTML export.
///
/// Use `#show math.equation: math-svg` to apply it to every equation. An inline equation stays in
/// its line, a display equation stays a block of its own: the equation's own `block` setting
/// decides. Options are checked for every export target, and outside HTML export the equation is
/// returned unchanged.
/// Inline equations align with surrounding text automatically.
///
/// The wrapper is the element your stylesheet selects: it carries `role="math"` and the class
/// `tola-math-inline` or `tola-math-block`. SVG paints with the colors the equation was compiled
/// with; set `fill` and `stroke` to `currentColor` on the shapes inside the wrapper to make it
/// follow the surrounding text color instead.
///
/// Example - render every equation, naming one alternative:
///
/// ```typst site
/// #import "@tola/web:0.0.0": math-svg
/// #show math.equation: math-svg
/// #document("index.html")[
///   #let wrapper = math-svg($x^2$, alt: "x squared")
///   #assert.eq(type(wrapper), content)
///   #wrapper
/// ]
/// ```
///
/// - equation (content): the equation to render.
/// - alt (auto | none | string): describes the equation for assistive technology: `auto` takes
///   the equation's own `alt`, `none` leaves the wrapper unnamed, and a string names it. It
///   becomes the wrapper's `aria-label`.
/// - attrs (dictionary): string-valued HTML attributes for the wrapper, such as `class` and
///   `style`. `role` and `aria-label` belong to the helper, and a `class` you pass keeps its
///   value and gains the wrapper's own token.
/// -> content
#let math-svg(equation, alt: auto, attrs: (:)) = context {
  import "web-fields.typ": math-attributes

  if type(equation) != content or equation.func() != math.equation {
    panic("math-svg expects a math.equation")
  }
  let block = equation.at("block", default: math.equation.block)
  let wrapper-class = if block { "tola-math-block" } else { "tola-math-inline" }
  let attributes = math-attributes(equation, alt, attrs, wrapper-class, "math-svg")
  if target() != "html" {
    return equation
  }
  if block {
    html.elem("div", attrs: attributes, html.frame(equation))
  } else {
    html.elem("span", attrs: attributes, box(html.frame(equation)))
  }
}

/// Declare one feed output through metadata.
///
/// Put the returned content in the root program: the declaration takes effect only there. The site
/// needs `site.origin`, because every feed address is absolute, and a non-blank feed title from
/// `title` or `site.title`.
///
/// An entry requires `target` and `published`, and may set `id`, `title`, `updated`, `summary`,
/// `content`, and `authors`. A `target` is an output path, an `(output:, fragment:)` dictionary, a
/// label, or a location, and the entry links to that document with the fragment the target names.
/// An `auto` or omitted `id` is the resolved target URL, and an `auto` or omitted `title` is the
/// target document's own title.
///
/// `summary` is the excerpt and `content` is the entry body, usually the full text. They are
/// independent: setting only `summary` publishes an excerpt, and naming `target` does not copy the
/// target document's body into the feed. Each format writes the two fields separately:
///
/// | Format | Summary | Content |
/// | --- | --- | --- |
/// | RSS | `<description>` | `<content:encoded>`, for full-text readers |
/// | Atom | `<summary>` | `<content>`: plain strings as `text`, fragments or selections as `html` |
/// | JSON Feed | `summary`, as plain text | Strings as `content_text`, fragments or selections as `content_html` |
///
/// An omitted or `none` body writes no RSS or Atom content element; JSON Feed writes an empty
/// `content_text`.
///
/// Each format fills in the modification date in its own way. Atom's `updated` defaults to
/// `published`. RSS carries no modification date. JSON Feed writes `date_modified` only when the
/// entry sets `updated`.
///
/// Strings are plain text: `"<strong>Note</strong>"` stays literal text. Typst content supports
/// text, breaks, emphasis, strong, strike, and URL links. Tola walks those authored elements to
/// make a portable HTML fragment; it runs no document show rules or layout. For example,
/// `content: [*Note*]` becomes `<strong>Note</strong>`, while `content: [= Heading]` raises an error.
/// Styled, contextual, or already processed content is also rejected. Use a document selection
/// for headings, tables, images, mathematics, or rendered code. Links accept relative paths and
/// fragments, absolute HTTP(S), `mailto`, and `tel` URLs. Relative links resolve against the
/// entry's target URL.
///
/// `content: (document: output, id: "post-body")` selects the final HTML element with that id and
/// its descendants. Omit `id` or pass `none` to select the whole body; `summary` accepts no
/// document selection.
/// Give the article an explicit HTML id to keep navigation and footers out. The id must occur
/// exactly once in the selected HTML document: an empty id, a missing or repeated id, or a
/// non-HTML document fails the build. An id in another output does not match.
///
/// The selection keeps ancestor containers around that subtree, without their other children,
/// and includes the head's styles and stylesheet links. HTML document containers become `div`s.
/// URL attributes resolve to absolute addresses under the original document's URL and first
/// `<base href>`. URLs inside CSS are not rewritten, and the full page's CSS cascade is not
/// reproduced.
///
/// The three ids have different purposes: the feed's `id` identifies the Atom feed, an entry's
/// `id` identifies that item to subscribers, and `content.id` selects an HTML element. Keep an
/// explicit entry id stable when a page moves. Entry ids must be non-blank and unique within the
/// feed. Atom requires absolute URI identifiers, and authors on the feed or on every entry of a
/// nonempty feed.
///
/// A label or location inside a document needs an exported anchor, so link to it somewhere in the
/// site (`#link(<label>)`, `@label`, or `#outline()`): the export gives an `id` only to an element
/// something links to. Without that anchor the build fails rather than linking the whole document.
/// A label or location of the whole document needs no body anchor. A `fragment` is kept as written
/// and must name an anchor the target document exports.
///
/// Typst datetime values are read as UTC, and RFC 3339 strings keep their own timezone.
///
/// Example - publish an RSS excerpt without a full-text body:
///
/// ```typst
/// #import "@tola/web:0.0.0": feed
/// #document("notes/index.html", title: [A note])[An idea and its explanation.]
/// #feed(
///   title: "Notes",
///   entries: ((
///     id: "urn:notes:first",
///     target: "notes/index.html",
///     published: datetime(year: 2026, month: 9, day: 1),
///     summary: "An idea in brief.",
///   ),),
/// )
/// ```
///
/// Example - publish an excerpt and full article HTML, leaving navigation out:
///
/// ```typst
/// #import "@tola/web:0.0.0": feed
/// #let output = "notes/index.html"
/// #document(output, title: [A note])[
///   #html.html[
///     #html.head(html.title("A note"))
///     #html.body[
///       #html.nav[Site navigation]
///       #html.article(id: "post-body")[
///         #title()
///         = A useful idea
///         Keep this paragraph in the feed.
///       ]
///     ]
///   ]
/// ]
/// #feed(
///   title: "Notes",
///   entries: ((
///     id: "urn:notes:first",
///     target: output,
///     published: datetime(year: 2026, month: 9, day: 1),
///     summary: [A useful idea in brief.],
///     content: (document: output, id: "post-body"),
///   ),),
/// )
/// ```
///
/// The starter's `site/seo.typ` maps source metadata `feed-summary` to an entry's `summary`, and
/// `feed-content` to its `content`. When `feed-content` is omitted or `none`, that recipe selects
/// the page's whole body. Setting only `feed-summary` therefore still publishes full-text RSS.
/// For an excerpt-only feed, change that recipe to pass `content: none`.
///
/// Example - give a starter page a separate, portable feed body:
///
/// In a site created with the `feed` feature, put this declaration at the start of
/// `content/note.typ`, followed by the page's normal body. Set `site.origin` and `site.title` for
/// the starter's feed output. `feed-content` below uses only the supported text fragment elements.
///
/// ```typst site
/// #import "@tola/source:0.0.0": tola-meta
/// #tola-meta((
///   title: "A note",
///   published: datetime(year: 2026, month: 9, day: 1),
///   feed-summary: [An idea in _brief_.],
///   feed-content: [*The full feed text.* Read #link("https://example.test/more/")[more].],
/// ))
/// ```
///
/// Related: sitemap
///
/// - output (auto | string): the output file, relative to the site output root; `auto` follows
///   `format`: `rss` gives `feed.xml`, `atom` gives `atom.xml`, and `json` gives `feed.json`.
/// - format (string): the feed format: `rss`, `atom`, or `json`.
/// - id (auto | string): the feed identifier; `auto` is the feed's own absolute URL.
/// - title (auto | content | string): the feed title; `auto` takes the site's title.
/// - description (auto | content | string): the feed description; `auto` takes the site's
///   description.
/// - language (auto | string): the feed language; `auto` takes the site's language tag.
/// - authors (auto | array): the feed authors; `auto` takes the site's authors.
/// - entries (array): the entries to publish, in the order you give them.
/// -> content
#let feed(
  output: auto,
  format: "rss",
  id: auto,
  title: auto,
  description: auto,
  language: auto,
  authors: auto,
  entries: (),
) = {
  import "@tola/address:0.0.0": output-to-url
  import "@tola/site:0.0.0": site

  let output = if output != auto {
    output
  } else if format == "rss" {
    "feed.xml"
  } else if format == "atom" {
    "atom.xml"
  } else if format == "json" {
    "feed.json"
  } else {
    panic("feed format must be `rss`, `atom`, or `json`")
  }
  let language = if language == auto { site.language.tag } else { language }
  [
    #metadata((
      output: output,
      format: format,
      id: if id == auto {
        output-to-url(output, origin: site.origin)
      } else { id },
      title: if title == auto { site.title } else { title },
      description: if description == auto { site.description } else { description },
      language: language,
      authors: if authors == auto { site.authors } else { authors },
      entries: entries,
    )) <tola-feed>
  ]
}

/// Declare one sitemap output through metadata.
/// Place the returned content in the root program, as for `feed`.
///
/// Every target names a whole document. A target that resolves to an element (a label or location
/// inside a document, or a dictionary with a `fragment`) raises an error, and so does the same
/// target twice. Every listed URL is absolute, so the site needs `site.origin`. Typst datetime
/// values are read as UTC, and RFC 3339 strings keep their own timezone.
///
/// Example - select whole documents, dating one of them:
///
/// ```typst
/// #import "@tola/web:0.0.0": sitemap
/// #document("index.html")[Home]
/// #document("guide/index.html")[Guide]
/// #sitemap(targets: (
///   "index.html",
///   (target: "guide/index.html", lastmod: datetime(year: 2026, month: 9, day: 1)),
/// ))
/// ```
///
/// Related: feed
///
/// - output (string): the output file, relative to the site output root.
/// - targets (array): the documents the sitemap lists, each entry an output path, a label, a
///   location, or a `(target:, lastmod:)` dictionary that dates it.
/// -> content
#let sitemap(output: "sitemap.xml", targets: ()) = [
  #metadata((
    output: output,
    targets: targets,
  )) <tola-sitemap>
]
