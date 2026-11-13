// @tola/source:0.0.0 - discovered content sources, the source a call runs in, and metadata declarations
//
// The entry file decides which pages to build from the discovered sources. It may use one
// source in several pages. The site defines fields such as `title`, `draft`, and `permalink`
// and decides how to use them.

/// List the Typst content files, sorted by path.
///
/// Tola looks under `build.content-dir` for files ending in lowercase `.typ`. It skips hidden
/// names, symbolic links, and the site entry. Sort the result yourself to order articles by date
/// or other metadata.
///
/// Each record includes:
///
/// - `id` and `path`: the same path string relative to `build.content-dir`, including the
///   extension, such as `posts/deep.typ` or `about/index.typ`. Both change when the source is
///   renamed or moved within the content tree.
/// - `file`: a Typst `path` value addressing the same file from the site root. Pass it directly to
///   `include` or `read`; it is an input path, never a browser URL.
/// - `filename`: the last component of `path`, such as `deep.typ`.
/// - `route-segments`: the parts of the default route, taken from the file path.
///   The home page has `()`. Both `about.typ` and `about/index.typ` have `("about",)`.
///   `posts/deep.typ` has `("posts", "deep")`.
/// - `meta`: the fields the source declares with `tola-meta`. Read metadata across
///   pages in `site.typ`, where the site has collected the declarations.
///
/// Store a fixed feed entry `id` in the metadata and reuse it after moving the page.
/// Feed readers can then recognize the same entry at its new URL.
///
/// `tola inspect sources` lists discovered file paths without evaluating metadata.
/// Use `tola query` to return selected metadata as JSON.
///
/// The segments only suggest a route. Slug and join them with `@tola/address`, and let the
/// document's own output decide which route reaches it.
///
/// Example: List an empty site's sources:
///
/// ```typst
/// #import "@tola/source:0.0.0": all-sources
/// #let sources = all-sources()
/// #assert.eq(sources.len(), 0)
/// ```
///
/// Example: Choose an article by its source path and publish it:
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

/// Read information about the source file containing this call.
///
/// Returns `id`, `file`, `path`, `filename`, and `route-segments`, with the same meanings
/// as in `all-sources()`. You can call it outside `context`. Use `all-sources()` for metadata.
///
/// Call it in the article and pass the result to your template. Calling it in the template
/// looks up the template file. A call outside a content source, such as in `site.typ`, raises an
/// error. To look up where an article is published, use `current-document()`.
///
/// Related: all-sources, tola-meta
#import "@tola/host:0.0.0": current-source

/// Declare the metadata of this source.
///
/// Call once per content file with a dictionary of metadata. You can call directly before
/// the body, or call a wrapper function imported from another file. Calls deferred
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
/// Example: Declare metadata and read its marker:
///
/// ```typst site
/// #import "@tola/source:0.0.0": tola-meta
/// #let marker = tola-meta((title: "Hello"))
/// #assert.eq(marker.value, (title: "Hello"))
/// #assert.eq(marker.label, <tola-meta>)
/// ```
///
/// Example: Wrap named metadata fields:
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

/// Validate source metadata and return the records with parsed `meta` values.
///
/// In `site.typ`, pass records from `all-sources()` and a schema from `@tola/schema`.
/// If pages need different fields, pass a function that takes a source record and returns
/// the schema for that page. Sources without metadata are checked as empty dictionaries.
///
/// The returned records keep their original order and paths. Use the returned `meta`
/// for filtering and templates. If validation fails, Tola reports the affected files
/// and points to their metadata or to the start of files with no declaration.
///
/// Example: Publish articles and skip drafts:
///
/// Start with a site created by `tola init`. Create `content/guide.typ` and `content/draft.typ`.
/// In each file, import the declaration function with `#import "@tola/source:0.0.0": tola-meta`.
/// In `guide.typ`, follow it with `#tola-meta((title: "Guide"))` and the body `= Guide`.
/// In `draft.typ`, use `#tola-meta((title: "Draft", draft: true))` and the body `= Draft`.
///
/// Save the following as `site.typ` and run `tola build` from the site root. The default output
/// includes `public/guide/index.html`. The draft is validated and then filtered out. Every other
/// file under `content/` also needs a nonempty `title`.
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
