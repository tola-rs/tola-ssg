// @tola/feed:0.0.0 - RSS, Atom, and JSON Feed outputs

/// Publish an RSS, Atom, or JSON Feed from the Typst entry file (`site.typ` in a new site).
///
/// #### Add an RSS feed
///
/// In `tola.toml`, set `origin = "https://example.com"` under `[site]` so Tola can write absolute
/// URLs. Add this to `site.typ`.
///
/// ```typst
/// #import "@tola/feed:0.0.0": feed
/// #document("notes/index.html", title: [A Typst note])[I use Typst to keep my notes in one place.]
/// #feed(
///   title: "Notes",
///   entries: ((
///     id: "urn:notes:first",
///     target: "notes/index.html",
///     published: datetime(year: 2026, month: 9, day: 1),
///     summary: "My first note on Typst.",
///   ),),
/// )
/// ```
///
/// Run `tola build`. With the default settings, `public/feed.xml` contains the excerpt and a
/// link to `https://example.com/notes/`.
///
/// #### Entries and formats
///
/// Set a non-blank `title` on the feed or in `[site]`. Each entry needs `target` and `published`.
///
/// `target` accepts an output path, an `(output:, fragment:)` dictionary, a label, or a location.
/// When omitted or set to `auto`, the entry's `id` defaults to the target URL and the entry's
/// `title` defaults to the target document's title. An entry can also set `updated`, `summary`,
/// `content`, and `authors`.
///
/// Set `summary` for an excerpt. Set `content` separately to include the body in the feed.
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
/// Atom's `updated` defaults to `published`. RSS omits the modification date. JSON Feed writes
/// `date_modified` when the entry sets `updated`.
///
/// #### Entry body
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
/// #### IDs, anchors, and dates
///
/// The feed's `id` identifies the Atom feed. An entry's `id` identifies the item to subscribers.
/// Keep explicit entry ids stable when pages move. Each id must be non-blank and unique within
/// the feed. Atom requires absolute URIs for feed and entry ids, and authors on the feed or on
/// every entry of a nonempty feed.
///
/// A label or location inside a document needs an exported anchor, so link to it somewhere in the
/// site (`#link(<label>)`, `@label`, or `#outline()`): the export gives an `id` only to an element
/// something links to. Without that anchor the build fails rather than linking the whole document.
/// A label or location of the whole document needs no body anchor. A `fragment` is kept as written
/// and must name an anchor the target document exports.
///
/// `published` and `updated` accept native datetimes with a complete calendar date, or strict
/// RFC 3339 strings with timezone, using the same parser as `@tola/schema`'s `timestamp`.
/// Native values use UTC; a native date without a time uses midnight. Date-only strings,
/// time-only values, surrounding whitespace, and leap seconds are rejected. Pass `.rfc3339`
/// when your schema returns a timestamp record. Feed formatting preserves the numeric offset
/// and at most nine fractional digits; RSS dates have second precision.
///
/// #### Include the article without the navigation
///
/// Replace the previous example with this version. `content.id` selects the `<article>`.
///
/// ```typst
/// #import "@tola/feed:0.0.0": feed
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
/// #### Write a separate body for feed readers
///
/// Put this declaration at the start of `content/note.typ`, followed by the page's normal body.
/// When building the entries in `site.typ`, pass `feed-summary` to `summary` and `feed-content`
/// to `content`. `tola-meta` stores the values for your entry-building code to read.
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
/// Related: @tola/sitemap, @tola/schema
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
  import "@tola/address:0.0.0": output-url
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
        output-url(output, origin: site.origin)
      } else { id },
      title: if title == auto { site.title } else { title },
      description: if description == auto { site.description } else { description },
      language: language,
      authors: if authors == auto { site.authors } else { authors },
      entries: entries,
    )) <tola-feed>
  ]
}
