// @tola/document:0.0.0 - the document you are in, its headings, and the references you wrote
//
// This package answers three questions about a page you are building. `current-document()` says
// which document a call belongs to, `headings()` lists that document's headings, and
// `references()` lists the links and refs your Bundle wrote. Reading a document needs
// document context, so call these inside `context`, in a `document(…)` body. Sources rendered
// into one document share that identity, while the identity of a single source lives in
// `@tola/source`.

/// The document whose output contains this call: its `output`, `route`, and `location`.
///
/// `output` is the document's output path in the Bundle, such as `notes/index.html`. `route` is
/// the site-root route it is reachable at, before a deployment path is applied. `location` is
/// the location of the enclosing document element, so you can pass it to `link` or to a query.
/// Call it inside `context`. A call outside a `document(…)` body raises an error. Two sources
/// rendered into one document report the same document.
///
/// Example - identify the containing document:
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
/// Example - scope a query to the containing document:
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

/// The links and refs you wrote in the Bundle, in Bundle order.
///
/// Call it inside `context`. Each document contributes its outermost native `link` and `ref`
/// occurrences, then every supplied filter applies to those occurrences. Distinct occurrences
/// remain distinct even when they point at the same target.
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
/// Each record describes one reference. A `ref` reports the label it names, and a `link` reports
/// the destination it spells. Show rules do not change that relation. Native destinations name
/// one target. A destination naming its own scheme and host is `external`, `site.origin`
/// included. External and unresolved references remain available without target filters.
///
/// | Field | Meaning |
/// | --- | --- |
/// | `element` | The original native `link` or `ref` element |
/// | `document` | The originating document's `output`, `route`, and `location` |
/// | `destination` | The original native destination value |
/// | `resolution` | `found`, `external`, or `unresolved` |
/// | `reason` | Why nothing resolved, as a dictionary; `none` when something did |
/// | `target` | The target dictionary, when its output or address is known; otherwise `none` |
///
/// A target carries `kind`, `output`, `route`, `query`, `fragment`, and `location`; a value it
/// cannot supply is `none`. `kind` is `output` for a native document or asset, `element` for
/// another native target, and `url` for a URL destination. Only native targets carry their actual
/// location. URL targets carry output and address information and have no location: they can
/// match `to` document or asset selections, but not element selections or `to-within`.
///
/// A URL's `fragment` is percent-decoded for display; a native target's anchor is kept as assigned.
/// Native whole-document and asset targets have no fragment. `found` means a native Bundle output
/// exists, whether or not the exported HTML holds the fragment. Raw HTML links and other
/// producers' outputs belong to final HTML validation, outside this query. Render a derived list
/// outside the region `from-within` reads so the list does not join its own query.
///
/// A `reason` carries a stable `tag` to branch on, a `message`, and a `help` naming the fix:
///
/// | Tag | Meaning |
/// | --- | --- |
/// | `outside-mount` | The destination lies outside `site.base-path` |
/// | `invalid-destination` | The destination is not a URL this site can read |
/// | `no-such-target` | No output of this build answers the destination |
/// | `positional-destination` | An element destination does not name a site target |
/// | `label-has-no-location` | The label's content has no location in this build |
/// | `ambiguous-label` | More than one element carries the label |
///
/// Example - list the pages that link to the page you are rendering:
///
/// ```typst site
/// #import "@tola/address:0.0.0": output-to-url, route, route-to-output
/// #import "@tola/document:0.0.0": references
/// #import "@tola/source:0.0.0": all-sources
///
/// #let guide = all-sources().at(1)
/// #let install = all-sources().at(2)
/// #let guide-output = route-to-output(route(guide.route-segments))
/// #let install-output = route-to-output(route(install.route-segments))
///
/// #document(guide-output, title: guide.meta.title)[
///   #html.main[#link(output-to-url(install-output))[Install]] <body>
/// ]
///
/// #document(install-output, title: install.meta.title)[
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
/// Example - point at one heading rather than its whole document:
///
/// ```typst site
/// #import "@tola/address:0.0.0": output-to-url, route, route-to-output
/// #import "@tola/document:0.0.0": references
/// #import "@tola/source:0.0.0": all-sources
///
/// #let page = all-sources().at(2)
/// #let output = route-to-output(route(page.route-segments))
///
/// #document(output, title: page.meta.title)[
///   #link(<intro>)[Jump to the introduction]
///   #link(output-to-url(output))[Whole page]
///   = Introduction <intro>
///   #context {
///     let intro = query(<intro>).first().location()
///     assert.eq(references(to: intro).len(), 1)
///     assert.eq(references(to: intro).first().target.fragment, "intro")
///   }
/// ]
/// ```
///
/// Example - read only the references one document writes:
///
/// ```typst site
/// #import "@tola/address:0.0.0": route, route-to-output
/// #import "@tola/document:0.0.0": references
/// #import "@tola/source:0.0.0": all-sources
///
/// #let guide = all-sources().at(1)
/// #let output = route-to-output(route(guide.route-segments))
///
/// #document(output, title: guide.meta.title)[
///   #html.main[#link("https://typst.app/")[Typst]] <body>
///   #context {
///     let written = references(from: auto, from-within: <body>)
///     assert.eq(written.len(), 1)
///     assert.eq(written.first().resolution, "external")
///   }
/// ]
/// ```
///
/// Example - references in A's body to native targets inside B's chapter:
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
/// Example - select several targets or the headings inside a region:
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
/// `outline()`. A document with no headings returns an empty array.
///
/// | Field | Meaning |
/// | --- | --- |
/// | `level` | The heading level declared in the source |
/// | `nesting` | Depth in the outline tree; `none` when not outlined |
/// | `number` | Rendered number: a string from a pattern, a callback's result, or `none` |
/// | `text` | Plain heading text before show rules |
/// | `label` | The label you wrote, or `none` |
/// | `location` | The native location to pass to `link` |
/// | `outlined` | Whether the heading participates in the outline |
///
/// The query is scoped to this document, so the result never mixes in another page's headings.
/// `location` is the value to pass to `link()`, and linking makes Typst generate an anchor for
/// the heading. A heading with a label uses that label as its anchor; otherwise Typst generates
/// `loc-N`. Call it inside `context`; every location it returns belongs to this compilation.
///
/// Example - link the outlined headings down to level 2:
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
/// Example - watch `nesting` and `label` follow the outline tree:
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
/// - depth (none | int): `none` for every level, or a positive integer to keep only headings
///   declared up to that level.
/// -> array
#let headings = {
  import "@tola/host:0.0.0": plain-text

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
        while ancestors.len() > 0 and (
          ancestors.last() >= heading.level
            or (skipped != none and ancestors.last() >= skipped)
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
    found.enumerate().map(((index, heading)) => (
      level: heading.level,
      nesting: nesting.at(index),
      number: if heading.numbering == none { none } else {
        numbering(heading.numbering, ..heading-counter.at(heading.location()))
      },
      text: plain-text(heading.body),
      label: heading.at("label", default: none),
      location: heading.location(),
      outlined: heading.outlined,
    ))
  }
}
