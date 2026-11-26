#show: package(
  summary: [Discover Typst content files, inspect the current source, and declare or validate page metadata.],
  related: ("@tola/address", "@tola/schema", "[build]"),
)

The site entry decides which discovered sources become pages and defines the meaning of site-specific metadata fields such as `title`, `draft`, or `permalink`.

#export(
  "all-sources",
  summary: [List all Typst content files under `build.content-dir`, sorted by relative path.],
  related: ("current-source", "tola-meta", "parse-sources", "@tola/address#join-route"),
)[
  Discovers files ending in lowercase `.typ` under `build.content-dir`, skipping hidden names, symlinks, and the site entry file. Sort the returned array yourself when ordering by date or metadata.

  #table(
    columns: 2,
    table.header([Record field], [Meaning]),
    [`id`, `path`], [Path relative to `build.content-dir` including `.typ` (for example `"posts/deep.typ"`). Both change when a file is moved or renamed; store a fixed `id` in metadata for stable feed entries.],
    [`file`], [Typst `path` rooted at the site directory; pass directly to `include` or `read`.],
    [`filename`], [Final path segment (for example `"deep.typ"`).],
    [`route-segments`], [Default route segments derived from the path (`()` for home, `("about",)` for both `about.typ` and `about/index.typ`).],
    [`meta`], [Metadata dictionary declared by `tola-meta`, collected across pages in `site.typ`.],
  )

  Run `tola inspect sources` to list discovered files without evaluating metadata, or `tola query` to export metadata as JSON.
]

#export(
  "current-source",
  summary: [Return `(id, file, path, filename, route-segments)` for the content source containing this call.],
  related: ("all-sources", "tola-meta", "@tola/document#current-document"),
)[
  Callable outside `context`. Call it inside an article and pass the result to your template—calling it directly inside a template outside `build.content-dir` raises an error. To look up where an article is published, use #ref("@tola/document#current-document").
]

#export(
  "tola-meta",
  summary: [Declare the metadata dictionary for the current content source file.],
  params: (
    payload: [Dictionary of metadata fields for this content file.],
  ),
  related: ("all-sources", "parse-sources"),
)[
  Call at most once per content file, directly near the top of the file or via an imported helper wrapper. Calls deferred inside `context` or selector show rules do not declare source metadata, and a top-level `#show:` runs after the body (so a body error can prevent it from executing). Syntax or runtime errors in any discovered file fail the build even if your site entry later filters that file out as a draft.

  Returns an invisible `<tola-meta>` metadata marker queryable with `query(<tola-meta>)`, though recording happens at call time regardless of whether the return value is inserted into content.

  #example(title: [Wrap `tola-meta` in a typed helper], check: "default")[
  ```typst
  #import "@tola/source:0.0.0": tola-meta

  #let post(..fields) = tola-meta(fields.named())
  ```
  ]
]

#export(
  "parse-sources",
  summary: [Validate source records with a `@tola/schema` rule and return records with parsed `meta` values.],
  params: (
    sources: [Array of source records from `all-sources()` or a filtered subset.],
    schema: [A `@tola/schema` declaration, or a function `(source) => schema` selecting a schema per source record.],
  ),
  related: ("@tola/schema#schema", "all-sources", "tola-meta"),
)[
  Sources without a `tola-meta` call are validated as empty dictionaries. Returned records preserve input order and paths; validation failures point diagnostics directly to the offending source file and metadata span.

  #example(title: [Validate metadata, filter drafts, and publish pages], check: "default")[
  ```typst
  #import "@tola/address:0.0.0": join-route, output-of, slugify
  #import "@tola/schema:0.0.0": non-empty, optional, schema, trim
  #import "@tola/source:0.0.0": all-sources, parse-sources

  #let page-schema = schema((
    title: non-empty(trim(str)),
    draft: optional(bool, default: false),
  ), unknown: "keep")

  #let pages = parse-sources(all-sources(), page-schema).filter(s => not s.meta.draft)
  #for page in pages {
    let output = output-of(join-route(page.route-segments.map(slugify)))
    document(output, format: "html", title: page.meta.title)[#include page.file]
  }
  ```
  ]
]
