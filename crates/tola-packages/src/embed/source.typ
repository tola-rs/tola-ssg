// @tola/source:0.0.0 - discovered content sources, the source a call runs in, and metadata declarations
//
// The entry file decides which pages to build from the discovered sources. It may use one
// source in several pages. The site defines fields such as `title`, `draft`, and `permalink`
// and decides how to use them.

/// Every content source the build discovered, ordered by its complete content path.
///
/// Discovery reads the `build.content-dir` directory and accepts only files whose extension is the
/// lowercase `.typ`. It skips hidden names (those beginning with a dot), symbolic links, and the
/// site entry. Directory indexes are ordinary sources in this ordering; sort explicitly for an
/// editorial order.
///
/// Each record carries:
///
/// - `id` and `path`: the same path string relative to `build.content-dir`, including the
///   extension, such as `posts/deep.typ` or `about/index.typ`. Both change when the source is
///   renamed or moved within the content tree.
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
/// For a feed entry that should keep its identity when a page moves, store a fixed ID in your
/// site's metadata and pass it explicitly as the entry's `id` in `@tola/feed`.
///
/// `tola inspect sources` lists discovered file paths without evaluating metadata.
/// Use a model query to return the metadata your site needs as JSON.
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
/// Create `content/guide/index.typ` with `= Guide` as its body. Save this example as
/// `site.typ` in a new site and run `tola build`. The default output is `public/guide/index.html`.
///
/// ```typst site
/// #import "@tola/source:0.0.0": all-sources
/// #let guide = all-sources().find(source => source.path == "guide/index.typ")
/// #document("guide/index.html", format: "html", title: "Guide")[
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
/// the site entry, is an error. A source rendered into another document still reports itself; only
/// the document containing it changes.
///
/// Related: all-sources, tola-meta
#import "@tola/host:0.0.0": current-source

/// Declare the metadata of this source.
///
/// The call is the declaration: pass a dictionary that describes the source. Call it once during
/// ordinary evaluation, directly or through a function imported from another file. Calls deferred
/// by `context` or selector show rules declare nothing. A `#show: ...` wrapper runs after its
/// trailing body has been evaluated, so a body error can prevent its declaration from running.
/// A declaration whose call already ran survives a later error; the source's errors still fail
/// the build, even when your site excludes that source as a draft.
///
/// The call returns an invisible `<tola-meta>` marker that carries the same dictionary, and
/// `query(<tola-meta>)` finds it. Where the marker ends up changes nothing: the declaration comes
/// from the call alone. A second call in one source is an error, because a source declares its
/// metadata once. An imported function declares for the source evaluating that call. Top-level
/// declarations in imported or included files remain those files' own declarations.
///
/// Example - declare metadata and read its marker:
///
/// ```typst site
/// #import "@tola/source:0.0.0": tola-meta
/// #let marker = tola-meta((title: "Hello"))
/// #assert.eq(marker.value, (title: "Hello"))
/// #assert.eq(marker.label, <tola-meta>)
/// ```
///
/// Example - wrap named metadata fields:
///
/// ```typst
/// #import "@tola/source:0.0.0": tola-meta
/// #let note(..fields) = tola-meta(fields.named())
/// ```
///
/// Save this definition in `site/metadata.typ`. In `content/post.typ`, import it with
/// `#import "../site/metadata.typ": note`, then declare with `#note(title: "Hello")`.
///
/// Related: all-sources, parse-sources
#import "@tola/host:0.0.0": tola-meta

/// Resolve source records against a declaration of their metadata fields.
///
/// Pass the complete records from `all-sources()`, or a subset of them, together with a
/// `@tola/schema` declaration of the metadata you expect. To choose different declarations for
/// different sources, pass a function instead: it receives the complete source record and returns
/// that source's declaration. The call resolves every record before it reports field issues,
/// including sources with different declarations. A field issue names the source whose metadata
/// failed and points at the matching literal value when its location is known, otherwise at the
/// actual `tola-meta` declaration, including inside an imported function. It points at the source's
/// file start when no metadata was declared. Wrapper arguments are not traced back to their
/// callers. Notes and nested causes remain separate diagnostic hints. An invalid declaration
/// reports its expected and actual type at the calling site; a
/// schema function returning an invalid declaration names the source it was choosing for. A callback
/// execution error keeps its own location and stops compilation. Put complete-set validation in
/// the root program: declarations may still be incomplete while the content sources themselves
/// are being evaluated.
///
/// Each returned record is the record you passed, with `meta` replaced by the value the
/// declaration resolved for it. A source that declares no metadata carries `meta: none`, which
/// this function alone reads as an empty dictionary. Resolved metadata is always a dictionary.
/// The input order and identity fields stay unchanged. Parsing does not write back to
/// `all-sources()` or the original metadata declarations.
///
/// Example - validate, select, and publish sources in the root program:
///
/// This site chooses `title` and `draft` as metadata fields, validates every source before
/// excluding drafts, and slugs the file-layout segments to choose each document's output.
///
/// ```typst site
/// #import "@tola/address:0.0.0": join-route, output-of, slugify
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
///   let output = output-of(join-route(page.route-segments.map(slugify)))
///   document(output, format: "html", title: page.meta.title)[#include page.file]
/// }
/// ```
/// Related: @tola/schema, all-sources, tola-meta
/// - sources (array): the records to resolve, from `all-sources()` or a subset of it.
/// - schema (type | dictionary | function): a metadata declaration, or a function from each complete source record to its declaration.
/// -> array
#let parse-sources = {
  import "@tola/host:0.0.0": parse-source, report-error, report-source-issues
  import "@tola/schema:0.0.0": issue, try-parse

  (sources, schema) => {
    let issues = ()
    let resolved = ()
    if type(sources) != array {
      report-error(
        "parse-sources `sources` must be an array; found `" + repr(type(sources)) + "`",
        hint: "Pass `all-sources()` or a subset of it",
      )
    }
    for (index, source) in sources.enumerate() {
      if not (type(source) == dictionary and "path" in source and type(source.path) == str and "meta" in source) {
        report-error(
          "parse-sources requires a complete source record at index " + str(index) + "; found `" + repr(source) + "`",
          hint: "Pass `all-sources()` or a subset of it",
        )
      }
      // One parse per record: the successful value is the one returned, never recomputed.
      let parsed = parse-source(source, schema, try-parse)
      // The record's own `path` is what the native resolves a file and declaration range from, so
      // a diagnostic never pairs a path with a file obtained separately.
      if parsed.ok and type(parsed.value) == dictionary {
        resolved.push(parsed.value)
        continue
      }
      let found = if parsed.ok {
        (
          issue(
            "metadata must resolve to a dictionary, not " + repr(type(parsed.value)),
            code: "schema.metadata",
          ),
        )
      } else {
        parsed.issues
      }
      for found-issue in found {
        issues.push((path: source.path, issue: found-issue))
      }
      resolved.push(none)
    }
    if issues.len() != 0 {
      report-source-issues(issues)
    }
    sources.enumerate().map(((index, source)) => source + (meta: resolved.at(index)))
  }
}
