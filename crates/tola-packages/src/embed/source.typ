// @tola/source:0.0.0 - discovered content sources, the source a call runs in, and metadata declarations
//
// A discovered source is an input, not a published page: one source can appear in several
// documents, or none. The root program reads the settled declarations, validates the metadata
// it needs, and chooses which documents to emit. Metadata fields and selection rules belong to
// your site; Tola gives no built-in meaning to `title`, `draft`, or `permalink`.

/// Every content source the build discovered, ordered by its complete content path.
///
/// Discovery reads the `build.content-dir` directory and accepts only files whose extension is the
/// lowercase `.typ`. It skips hidden names (those beginning with a dot), symbolic links, and the
/// site entry. Directory indexes are ordinary sources in this ordering; sort explicitly for an
/// editorial order.
///
/// Each record carries:
///
/// - `id` and `path`: the complete path of the source below `build.content-dir`, extension included,
///   such as `posts/deep.typ`. The directory index of `about` is `about/index.typ`.
/// - `file`: a Typst `path` value addressing the same file from the site root. Pass it directly to
///   `include` or `read`; it is an input path, never a browser URL.
/// - `filename`: the last component of `path`, such as `deep.typ`.
/// - `route-segments`: the identity segments the file layout gives the source. A root index has
///   `()`, both `about.typ` and `about/index.typ` have `("about",)`, and `posts/deep.typ` has
///   `("posts", "deep")`.
/// - `meta`: the metadata the source declares, as this evaluation round sees it. A source whose
///   metadata reads other sources settles over several rounds, so its value can differ from one
///   round to the next. The root program receives the settled values.
///
/// `tola inspect sources` reads the metadata each source declares, without building the site.
///
/// The segments only suggest a route. Slug and join them with `@tola/address`, and let the
/// document's own output decide which route reaches it.
///
/// Example - list an empty site's sources:
///
/// ```typst
/// #import "@tola/source:0.0.0": all-sources
/// #let sources = all-sources()
/// #assert.eq(sources.len(), 0)
/// ```
///
/// Example - publish a source selected by its path:
///
/// Here `content/guide/index.typ` declares a `title` and contains the page's body.
///
/// ```typst site
/// #import "@tola/source:0.0.0": all-sources
/// #let guide = all-sources().find(source => source.path == "guide/index.typ")
/// #document("guide/index.html", format: "html", title: guide.meta.title)[
///   #include guide.file
/// ]
/// ```
/// Related: current-source, tola-meta, parse-sources, @tola/address
#import "@tola/host:0.0.0": all-sources

/// The source that this call runs in.
///
/// The record carries `id`, `file`, `path`, `filename`, and `route-segments`, exactly as a record
/// of `all-sources()` does. It carries no `meta`, and you can read it during ordinary evaluation,
/// without `context`.
///
/// Call it in the content source itself, then pass the record to your templates. A call written in
/// another file names that file instead. A call in a file that is not a content source, such as
/// the site entry, is an error.
///
/// Related: all-sources, tola-meta
#import "@tola/host:0.0.0": current-source

/// Declare the metadata of this source.
///
/// The call is the declaration: pass a dictionary that describes the source. Write it once,
/// directly in the source's ordinary evaluation, outside deferred `context` or show rules. If
/// later evaluation fails, Tola still reads a declaration whose call already ran; the source's
/// errors still fail the build, even when your site excludes that source as a draft.
///
/// The call returns an invisible `<tola-meta>` marker that carries the same dictionary, and
/// `query(<tola-meta>)` finds it. Where the marker ends up changes nothing: the declaration comes
/// from the call alone. A second call in one source is an error, because a source declares its
/// metadata once. Only a call written in the source declares. A call that a helper in another
/// file makes belongs to that file, so it declares nothing here.
///
/// Example - declare metadata and read its marker:
///
/// ```typst site
/// #import "@tola/source:0.0.0": tola-meta
/// #let marker = tola-meta((title: "Hello"))
/// #assert.eq(marker.value, (title: "Hello"))
/// #assert.eq(marker.label, <tola-meta>)
/// ```
/// Related: all-sources, parse-sources
#import "@tola/host:0.0.0": tola-meta

/// Resolve source records against a declaration of their metadata fields.
///
/// Pass the complete records from `all-sources()`, or a subset of them, together with a
/// `@tola/schema` declaration of the metadata you expect. The call resolves every record before it
/// reports, so one build names every source that disagrees with the declaration. Each diagnostic
/// points at that source's `tola-meta` call, or the file start when it declares no metadata. Put
/// complete-set validation in the root program: declarations may still be incomplete while the
/// content sources themselves are being evaluated.
///
/// Each returned record is the record you passed, with `meta` replaced by the value the
/// declaration resolved for it. A source that declares no metadata carries `meta: none`, which
/// this function alone reads as an empty dictionary. Resolved metadata is always a dictionary.
/// The input order and identity fields stay unchanged. Parsing does not write back to
/// `all-sources()` or the declarations that `tola inspect sources` reads.
///
/// Example - validate, select, and publish sources in the root program:
///
/// This site chooses `title` and `draft` as metadata fields, validates every source before
/// excluding drafts, and slugs the file-layout segments to choose each document's output.
///
/// ```typst site
/// #import "@tola/address:0.0.0": route, route-to-output, slugify
/// #import "@tola/schema:0.0.0": non-empty, optional, schema, trim
/// #import "@tola/source:0.0.0": all-sources, parse-sources
///
/// #let page-schema = schema((
///   title: non-empty(trim(str)),
///   draft: optional(bool, default: false),
/// ), unknown: "keep")
/// #let pages = {
///   parse-sources(all-sources(), page-schema)
///     .filter(source => not source.meta.draft)
/// }
/// #for page in pages {
///   let output = route-to-output(route(page.route-segments.map(slugify)))
///   document(output, format: "html", title: page.meta.title)[#include page.file]
/// }
/// ```
/// Related: @tola/schema, all-sources, tola-meta
/// - sources (array): the records to resolve, from `all-sources()` or a subset of it.
/// - schema (type | dictionary): the declaration every record's `meta` must satisfy.
/// -> array
#let parse-sources = {
  import "@tola/host:0.0.0": report-source-issues
  import "@tola/schema:0.0.0": format-issues, issue, try-parse

  (sources, schema) => {
    let issues = ()
    let resolved = ()
    assert(type(sources) == array, message: "parse-sources expects an array from all-sources()")
    for (index, source) in sources.enumerate() {
      assert(
        type(source) == dictionary and "path" in source and type(source.path) == str and "meta" in source,
        message: "parse-sources expects the complete records from all-sources(); the value at index "
          + str(index)
          + " is not one. Pass `all-sources()` or a subset of it",
      )
      // One parse per record: the successful value is the one returned, never recomputed.
      let metadata = if source.meta == none { (:) } else { source.meta }
      let parsed = try-parse(metadata, schema)
      // The record's own `path` is what the native resolves a file and declaration range from, so
      // a diagnostic never pairs a path with a file obtained separately.
      if parsed.ok and type(parsed.value) == dictionary {
        resolved.push(parsed.value)
        continue
      }
      let found = if parsed.ok {
        (issue(
          "metadata must resolve to a dictionary, not " + repr(type(parsed.value)),
          code: "schema.metadata",
        ),)
      } else {
        parsed.issues
      }
      for found-issue in found {
        issues.push((path: source.path, message: format-issues((found-issue,))))
      }
      resolved.push(none)
    }
    if issues.len() != 0 {
      report-source-issues(issues)
    }
    sources.enumerate().map(((index, source)) => source + (meta: resolved.at(index)))
  }
}
