#import "@tola/address:0.0.0": decode-url-path, route, route-to-output, slugify
#import "@tola/site:0.0.0": site
#import "@tola/source:0.0.0": parse-sources
#import "schema.typ": page-schema

/// The decoded site-root route one source publishes at: its `permalink` when it sets one, and
/// otherwise the route its file layout gives it, each segment slugged for the site's language.
///
/// A trailing `/` names a directory: `/about/` publishes `about/index.html`.
///
/// - source (dictionary): one source record, as `parse-sources` returns it.
/// -> string
#let source-route(source) = {
  if source.meta.permalink == none {
    route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))
  } else {
    decode-url-path(source.meta.permalink)
  }
}

/// Select the pages to publish: one entry per source that is not a draft, pairing the source with
/// the output file its route names.
///
/// - sources (array): complete source records from `all-sources()`.
/// -> array
#let select-pages(sources) = {
  // Every source is validated here, drafts included, before choosing which to publish.
  parse-sources(sources, page-schema)
    .filter(source => not source.meta.draft)
    .map(source => (
      source: source,
      output: route-to-output(source-route(source)),
    ))
}
