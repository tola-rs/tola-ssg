#show: package(
  summary: [Inspect the current document, list its headings, query cross-document links, and extract plain text.],
  related: ("@tola/source", "@tola/address"),
)

Call `current-document`, `headings`, and `references` inside a `context` block within a `#document(...)` body. Import specific members (or alias the package) so `@tola/document` does not shadow Typst's built-in `document(...)` function.

#export(
  "current-document",
  summary: [Return metadata about the enclosing `#document(...)` inside `context`.],
  related: ("headings", "references", "@tola/source#current-source"),
)[
  Returns a dictionary with:
  - `output`: publish-relative output path (for example `"notes/index.html"`).
  - `route`: canonical site-root route without `base-path` (for example `"/notes/"`).
  - `location`: Typst `location` of the document, useful with `link(...)` or `query(selector(...).within(current-document().location))`.

  #example(title: [Scope a Typst query to the current document], check: "default")[
  ```typst
  #import "@tola/document:0.0.0": current-document

  #document("index.html")[
    = Overview
    #context {
      let here = current-document()
      let local-headings = query(selector(heading).within(here.location))
      [Route: #here.route (#local-headings.len() heading)]
    }
  ]
  ```
  ]
]

#export(
  "references",
  summary: [Query Typst `link` and `ref` elements across the bundle in document order (for example backlinks).],
  params: (
    from: [Restrict to links inside documents containing the selected elements (`auto` selects the current document; `none` searches all documents).],
    from-within: [Restrict to links occurring inside the selected ancestor elements.],
    to: [Restrict to links targeting the selected documents, assets, or elements (`auto` selects the current document).],
    to-within: [Restrict to links targeting Typst elements inside the selected ancestor regions.],
  ),
  related: ("current-document", "headings", "@tola/address#output-url"),
)[
  Call inside `context`. Each filter accepts a label, location, locatable element function, selector, `auto` (for `from`/`to`), or `none`. All supplied filters must match. Only outermost `link`/`ref` elements surviving show rules are returned; handwritten HTML links are not included (use `tola check` to validate final HTML links).

  #table(
    columns: 2,
    table.header([Result field], [Meaning]),
    [`element`], [The post-show-rule `link` or `ref` element.],
    [`document`], [Source document's `(output, route, location)` record.],
    [`destination`], [Target declared on the element.],
    [`resolution`], [`"found"`, `"external"`, or `"unresolved"`.],
    [`reason`], [Present when `"unresolved"`: `(tag, message, help)` (`outside-mount`, `invalid-destination`, `no-such-target`, `positional-destination`, `label-has-no-location`, `ambiguous-label`).],
    [`target`], [Target record `(kind, output, route, query, fragment, location)` (`kind` is `"output"`, `"element"`, or `"url"`), or `none`.],
  )

  If you render new links from the query results, place them outside any `from-within` region you query so the query remains stable.

  #example(title: [List pages linking to the current document], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": output-url
  #import "@tola/document:0.0.0": references

  #document("a.html", title: [A])[#link(output-url("b.html"))[See B]]
  #document("b.html", title: [B])[
    #context {
      let incoming = references(to: auto)
      for item in incoming [Linked from #item.document.route]
    }
  ]
  ```
  ]
]

#export(
  "headings",
  summary: [Return the headings of the current document in document order.],
  params: (
    depth: [Optional positive integer capping effective `level` (after `heading.offset`). Omit or pass `none` to return all headings.],
  ),
  related: ("current-document", "references"),
)[
  Call inside `context`. Lists every heading in the current document, including those with `outlined: false` (filter by `h.outlined` to match Typst's `outline()`).

  #table(
    columns: 2,
    table.header([Field], [Meaning]),
    [`level`], [Effective heading level including `heading.offset`.],
    [`nesting`], [Outline nesting depth, or `none` when `outlined` is `false`.],
    [`number`], [Formatted numbering result, or `none`.],
    [`body` / `text`], [Original heading content and plain text before show rules.],
    [`label`], [Attached `<label>`, or `none`.],
    [`location`], [Typst `location`; passing it to `link(...)` causes HTML export to emit an anchor ID.],
    [`outlined`], [Whether the heading participates in outlines.],
  )

  #example(title: [Build a two-level table of contents], check: "default")[
  ```typst
  #import "@tola/document:0.0.0": headings

  #document("index.html")[
    #context {
      let toc = headings(depth: 2).filter(h => h.outlined)
      for h in toc [#link(h.location, h.body) ]
    }
    = Introduction
    == Details
  ]
  ```
  ]
]

#export(
  "plain-text",
  summary: [Extract plain text from a string or Typst content without running layout or show rules.],
  params: (
    value: [String or Typst content to flatten into plain text.],
  ),
)[
  Preserves text, symbols, spaces, raw code, and smart quotes; descends into formatting, links, and math; skips metadata and empty non-text elements.

  #example(title: [Flatten rich Typst content], check: "default")[
  ```typst
  #import "@tola/document:0.0.0": plain-text

  #let text = plain-text([Hello, *world*!])
  ```
  ]
]
