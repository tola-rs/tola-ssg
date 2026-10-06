// @tola/source:0.0.0 - discovered content sources, the source a call runs in, and metadata declarations

/// Every content source the build discovered, in discovery order.
///
/// Discovery reads the `build.content-dir` directory and accepts only files whose extension is the
/// lowercase `.typ`. A name that begins with a dot is hidden, and discovery passes over it. A
/// directory index is named after its directory.
///
/// Each record carries:
///
/// - `id` and `path`: the complete path of the source below `build.content-dir`, extension included,
///   such as `posts/deep.typ`. The directory index of `about` is `about/index.typ`.
/// - `file`: the same file addressed from the site root. It is the input path that `include` and
///   `read` take, never a browser URL.
/// - `filename`: the last component of `path`, such as `deep.typ`.
/// - `route-segments`: the identity segments the file layout gives the source. A root index has
///   `()`, both `about.typ` and `about/index.typ` have `("about",)`, and `posts/deep.typ` has
///   `("posts", "deep")`.
/// - `meta`: the metadata the source declares, as this evaluation round sees it. A source whose
///   metadata reads other sources settles over several rounds, so its value can differ from one
///   round to the next.
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
/// #assert.eq(sources.map(source => source.path), ())
/// ```
///
/// Example - read a site's source records:
///
/// ```typst site
/// #import "@tola/source:0.0.0": all-sources
/// #let sources = all-sources()
/// #assert.eq(sources.map(source => source.path), (
///   "index.typ",
///   "guide/index.typ",
///   "guide/install.typ",
///   "guide/deploy.typ",
///   "guide/advanced/plugins.typ",
///   "notes/first-post.typ",
///   "notes/second-post.typ",
/// ))
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
/// The call is the declaration: pass a dictionary that describes the source. A build reads it even
/// when the rest of the source fails, so the source still contributes the fields it declared.
/// Write the call once, directly in the source.
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
/// points at the `<tola-meta>` declaration of the source that raised it.
///
/// Each returned record is the record you passed, with `meta` replaced by the value the
/// declaration resolved for it. A source that declares no metadata carries `meta: none`, which
/// this function alone reads as an empty dictionary. Resolved metadata is always a dictionary.
///
/// Example - resolve metadata through a schema:
///
/// ```typst
/// #import "@tola/source:0.0.0": parse-sources
/// #import "@tola/schema:0.0.0": schema
/// #let records = ((path: "posts/deep.typ", meta: (title: "Deep notes")),)
/// #let parsed = parse-sources(records, schema((title: str)))
/// #assert.eq(parsed.first().meta, (title: "Deep notes"))
/// ```
///
/// Example - resolve every source of a site, keeping fields you do not declare:
///
/// ```typst site
/// #import "@tola/schema:0.0.0": array-of, optional, schema
/// #import "@tola/source:0.0.0": all-sources, parse-sources
/// #let pages = parse-sources(all-sources(), schema((
///   title: str,
///   tags: optional(array-of(str), default: ()),
/// ), unknown: "keep"))
/// #assert.eq(pages.len(), 7)
/// #assert.eq(pages.last().meta.tags, ("web", "typst"))
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
