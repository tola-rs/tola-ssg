// @tola/sitemap:0.0.0 - sitemap outputs for compiled documents

/// Write a sitemap for the documents you list.
///
/// Each target must resolve to a different whole document. A target inside a document or a
/// target dictionary with a `fragment` raises an error.
///
/// `lastmod` accepts a Typst date, a `YYYY-MM-DD` string, or a timestamp accepted by
/// `@tola/schema`. Dates stay date-only. Typst datetimes use UTC. Timestamp strings retain
/// their UTC offset and up to nine fractional digits. For a parsed timestamp, pass `.rfc3339`.
///
///
/// Set `origin = "https://example.com"` under `[site]` in `tola.toml`. Add this to the Typst
/// entry file (`site.typ` by default). If those pages already exist, add only the import
/// and the `#sitemap` call.
///
/// ```typst
/// #import "@tola/sitemap:0.0.0": sitemap
/// #document("index.html")[Home]
/// #document("guide/index.html")[Guide]
/// #sitemap(targets: (
///   "index.html",
///   (target: "guide/index.html", lastmod: datetime(year: 2026, month: 9, day: 1)),
/// ))
/// ```
///
/// Run `tola build` from the site directory. With the defaults, `public/sitemap.xml` lists
/// `https://example.com/` and `https://example.com/guide/`, with a last-modified date for the guide.
///
/// Related: @tola/feed, @tola/schema
///
/// - output (string): the output path relative to the publish directory, `sitemap.xml` by default.
/// - targets (array): the documents the sitemap lists, each entry an output path, a label, a
///   location, or a `(target:, lastmod:)` dictionary. If omitted, write an empty sitemap.
/// -> content
#let sitemap(output: "sitemap.xml", targets: ()) = [
  #metadata((
    output: output,
    targets: targets,
  )) <tola-sitemap>
]
