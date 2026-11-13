// @tola/document:0.0.0 - the document you are in, its headings, and the references you wrote
//
// Call `current-document()`, `headings()`, and `references()` in `context` inside a
// `document(…)` body. Sources rendered in the same document read the same document. To read
// the original source file’s path or metadata, use `@tola/source`.

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
/// Example: Read text from a string and from content:
///
/// ```typst
/// #import "@tola/document:0.0.0": plain-text
/// #assert.eq(plain-text("Guide"), "Guide")
/// #assert.eq(plain-text([Hello, *world*!]), "Hello, world!")
/// ```
///
/// Example: Extract text from an HTML image element:
///
/// ```typst
/// #import "@tola/document:0.0.0": plain-text
/// #let image = html.img(src: "/cover.png", alt: "Cover")
/// #assert.eq(plain-text(image), "")
/// ```
///
/// Related: headings, @tola/head
#import "@tola/host:0.0.0": plain-text

/// Return the document that contains this call.
///
/// Call it inside `context` in a `document(…)` body. The returned `output` is the file path,
/// such as `notes/index.html`. The `route` is the site route, such as `/notes/`, before the
/// deployment prefix. Pass `location` to `link` or `query`.
///
/// Example: Read the current document's output path and route:
///
/// ```typst
/// #import "@tola/document:0.0.0": current-document
/// #document("index.html")[
///   #context {
///     let document = current-document()
///     assert.eq(document.output, "index.html")
///     assert.eq(document.route, "/")
///   }
/// ]
/// ```
///
/// Example: Query headings in the current document:
///
/// ```typst
/// #import "@tola/document:0.0.0": current-document
/// #document("index.html")[
///   = Intro
///   #context {
///     let sections = query(selector(heading).within(current-document().location))
///     assert.eq(sections.len(), 1)
///   }
/// ]
/// ```
/// Related: references, headings, @tola/source
#import "@tola/host:0.0.0": current-document

/// Find the Typst links and references that appear in the site's documents, in Bundle order.
///
/// Call it inside `context`. The result lists the outermost `link` and `ref` elements after show
/// rules run. Links replaced with text or raw HTML are skipped. Links to the same target still
/// get separate results when they appear in different places.
///
/// | Parameter | Selects |
/// | --- | --- |
/// | `from` | Documents containing the selected elements |
/// | `from-within` | Ancestor regions containing the reference occurrence |
/// | `to` | Native document or asset outputs, or exact native target elements |
/// | `to-within` | Ancestor regions containing a known native target |
///
/// All four accept `none` (the default, no restriction), a label, a location, a locatable element
/// function, or a native locatable selector. Compose selections with `selector(...).or(...)`
/// or `.and(...)`. Labels select every matching element; a selection with no matches returns an
/// empty array. A single occurrence matching several alternatives appears once.
///
/// `from` selects the document containing each matched element; selecting an asset or content
/// outside every document is an error. `to` selects document and asset targets by output, and
/// other elements by their exact native location. `auto` is accepted by `from` and `to` and
/// selects the current document; it needs a call inside a document body.
///
/// Both region filters select strict descendants, excluding the region itself. A heading label
/// selects the heading element, not the chapter text following it; label a containing element
/// such as `html.section` to select that text. Every supplied condition must match.
///
/// Each record describes a link after show rules run. A `ref` reports its label. A `link`
/// reports its destination, including when a show rule replaces a `ref` with a `link`.
/// A Typst reference must have a unique target. A URL with a scheme and host counts as
/// `external`, even if it uses the site's origin. Omit target filters to include external
/// links and links whose destination could not be found.
///
/// | Field | Meaning |
/// | --- | --- |
/// | `element` | The native `link` or `ref` element that reached the output, after show rules |
/// | `document` | The originating document's `output`, `route`, and `location` |
/// | `destination` | The destination that element carried, after show rules |
/// | `resolution` | `found`, `external`, or `unresolved` |
/// | `reason` | Why the destination could not be found, or `none` when it was found |
/// | `target` | The target dictionary, when its output or address is known; otherwise `none` |
///
/// A target carries `kind`, `output`, `route`, `query`, `fragment`, and `location`; a value it
/// cannot supply is `none`. `kind` is `output` for a native document or asset, `element` for
/// another native target, and `url` for a URL destination. Only native targets carry their actual
/// location. URL targets carry output and address information and have no location: they can
/// match `to` document or asset selections, but not element selections or `to-within`.
///
/// A URL's `fragment` is percent-decoded for display; a native target's anchor is kept as assigned.
/// Whole-document and asset targets have `fragment: none`. `found` means the destination
/// was found in the Typst Bundle. Run `tola check` to check the final HTML, including HTML
/// links you wrote directly and fragments in the exported pages. Keep the links you generate
/// from this query outside the region passed to `from-within`.
///
/// Use `reason.tag` to test why a link failed. Show `reason.message` for the explanation and `reason.help` for the suggested fix.
///
/// | Tag | Meaning |
/// | --- | --- |
/// | `outside-mount` | The destination lies outside `site.base-path` |
/// | `invalid-destination` | The destination is not a URL this site can read |
/// | `no-such-target` | No matching output in this build |
/// | `positional-destination` | The link points to a page position. Use a URL, label, or element location |
/// | `label-has-no-location` | The label's content has no location in this build |
/// | `ambiguous-label` | More than one element carries the label |
///
/// Example: List the pages that link to the page you are rendering:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-url
/// #import "@tola/document:0.0.0": references
///
/// #let guide-output = "guide/index.html"
/// #let install-output = "guide/install/index.html"
///
/// #document(guide-output, title: [Guide])[
///   #html.main[#link(output-url(install-output))[Install]] <body>
/// ]
///
/// #document(install-output, title: [Install])[
///   #html.main[Install] <body>
///   #context {
///     let incoming = references(to: auto, from-within: <body>)
///     assert.eq(incoming.len(), 1)
///     assert.eq(incoming.first().document.output, guide-output)
///     for reference in incoming [
///       #link(reference.document.location)[From #reference.document.route]
///     ]
///   }
/// ]
/// ```
///
/// Example: Find references to a specific heading:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-url
/// #import "@tola/document:0.0.0": references
///
/// #let output = "guide/install/index.html"
///
/// #document(output, title: [Install])[
///   #link(<intro>)[Jump to the introduction]
///   #link(output-url(output))[Whole page]
///   = Introduction <intro>
///   #context {
///     let intro = query(<intro>).first().location()
///     assert.eq(references(to: intro).len(), 1)
///     assert.eq(references(to: intro).first().target.fragment, "intro")
///   }
/// ]
/// ```
///
/// Example: Find links from one document:
///
/// ```typst
/// #import "@tola/document:0.0.0": references
///
/// #document("guide/index.html", title: [Guide])[
///   #html.main[#link("https://typst.app/")[Typst]] <body>
///   #context {
///     let written = references(from: auto, from-within: <body>)
///     assert.eq(written.len(), 1)
///     assert.eq(written.first().resolution, "external")
///   }
/// ]
/// ```
///
/// Example: Find links to a chapter in another document:
///
/// ```typst
/// #import "@tola/document:0.0.0": references
/// #document("a.html")[
///   #html.main[
///     #link(<detail>)[Detail]
///     #link(<chapter>)[Chapter]
///   ] <a-body>
///   #context {
///     let found = references(
///       from: <a>, from-within: <a-body>,
///       to: <b>, to-within: <chapter>,
///     )
///     assert.eq(found.len(), 1)
///   }
/// ] <a>
/// #document("b.html")[
///   #html.section[
///     = Detail <detail>
///   ] <chapter>
/// ] <b>
/// ```
///
/// Example: Select several targets or the headings inside a region:
///
/// ```typst
/// #import "@tola/document:0.0.0": references
/// #document("index.html")[
///   #link(<first>)[First]
///   #link(<second>)[Second]
///   #context {
///     let targets = selector(<first>).or(<second>)
///     assert.eq(references(from: auto, to: targets).len(), 2)
///     let topics = selector(heading).within(<topics>)
///     assert.eq(references(from: auto, to: topics).len(), 2)
///   }
/// ]
/// #document("topics.html")[
///   #html.section[
///     = First <first>
///     = Second <second>
///   ] <topics>
/// ]
/// ```
///
/// Related: current-document, headings
#import "@tola/host:0.0.0": references

/// The headings of the current document, in document order.
///
/// Every heading is listed, including ones that `outlined: false` keeps out of Typst's
/// `outline()`. Filter by `outlined` when building a table of contents that follows `outline()`.
/// A document with no headings returns an empty array.
///
/// The records describe native heading elements, independently of their final HTML representation.
/// A show rule that returns plain text, an HTML wrapper, or nothing preserves the original
/// heading's record.
///
/// | Field | Meaning |
/// | --- | --- |
/// | `level` | The heading's resolved level: its declared depth plus the `offset` in effect |
/// | `nesting` | Level in the outline, or `none` if the heading is excluded |
/// | `number` | Rendered number: a string from a pattern, a callback's result, or `none` |
/// | `body` | The heading's source content before show rules |
/// | `text` | Plain heading text before show rules |
/// | `label` | The label you wrote, or `none` |
/// | `location` | The native location to pass to `link` |
/// | `outlined` | Whether the heading participates in the outline |
///
/// `level` includes any offset. For example, `set heading(offset: 2)` makes a heading written
/// with `==` report level 4.
///
/// `headings()` looks only in the current document. To limit any other query to this document, use
/// `query(selector(…).within(current-document().location))`.
///
/// `location` is the value to pass to `link()`, and linking makes Typst generate an anchor for
/// the heading. A heading with a label uses that label as its anchor; otherwise Typst generates
/// `loc-N`. Typst assigns anchors on demand, so a heading that nothing links to has no anchor id
/// in the exported HTML. Call it inside `context`; every location it returns belongs to this
/// compilation.
///
/// Example: Build a table of contents for the current document:
///
/// Save this as `site.typ` in a new site and run `tola build`. The home page links to
/// “Introduction” and “Details”. Level 3 and headings with `outlined: false` are omitted.
///
/// ```typ
/// #import "@tola/document:0.0.0": headings
/// #document("index.html")[
///   #context {
///     let sections = headings(depth: 2).filter(section => section.outlined)
///     assert.eq(sections.map(section => section.text), ("Introduction", "Details"))
///     for section in sections [#link(section.location, section.text)]
///   }
///   = Introduction
///   == Details
///   === Deep section
///   #heading(outlined: false)[Hidden section]
/// ]
/// #document("other.html")[= Another document]
/// ```
///
/// Example: Inspect `nesting` and `label` in the outline:
///
/// ```typst
/// #import "@tola/document:0.0.0": headings
/// #document("index.html")[
///   = Intro <intro>
///   #heading(level: 3, outlined: false)[Aside]
///   == Detail
///   #context {
///     let sections = headings()
///     assert.eq(sections.map(section => section.label), (<intro>, none, none))
///     assert.eq(sections.map(section => section.nesting), (1, none, 2))
///   }
/// ]
/// ```
///
/// Related: current-document
/// - depth (none | int): If omitted or `none`, include all headings. Pass a positive integer to keep headings
///   whose effective `level` is at most that number. This includes any `heading` offset:
///   a `==` heading with an offset of 1 has level 3, so `depth: 2` omits it.
/// -> array
#let headings = {
  // Bound outside the loop below, whose `heading` would shadow the element function.
  let heading-counter = counter(heading)

  // `outline()`'s tree: a heading nests under the deepest ancestor that is shallower than it,
  // and shallower than the most recent heading `outline()` skips.
  let nesting-of(found) = {
    let nesting = ()
    let ancestors = ()
    let skipped = none
    for heading in found {
      if heading.outlined {
        while (
          ancestors.len() > 0
            and (
              ancestors.last() >= heading.level or (skipped != none and ancestors.last() >= skipped)
            )
        ) {
          ancestors = ancestors.slice(0, ancestors.len() - 1)
        }
        nesting.push(ancestors.len() + 1)
        ancestors.push(heading.level)
        skipped = none
      } else {
        nesting.push(none)
        if skipped == none or heading.level < skipped { skipped = heading.level }
      }
    }
    nesting
  }

  (depth: none) => {
    assert(
      depth == none or (type(depth) == int and depth > 0),
      message: "`depth` must be `none` or a positive integer",
    )
    let ancestor = current-document().location
    let found = query(selector(heading).within(ancestor))
    if depth != none {
      found = found.filter(heading => heading.level <= depth)
    }
    let nesting = nesting-of(found)
    found
      .enumerate()
      .map(((index, heading)) => (
        level: heading.level,
        nesting: nesting.at(index),
        number: if heading.numbering == none { none } else {
          numbering(heading.numbering, ..heading-counter.at(heading.location()))
        },
        body: heading.body,
        text: plain-text(heading.body),
        label: heading.at("label", default: none),
        location: heading.location(),
        outlined: heading.outlined,
      ))
  }
}
