#import "@tola/source:0.0.0": parse-sources
#import "@tola/address:0.0.0": slugify, route, route-to-output, decode-url-path
#import "schema.typ": page-schema

#let select-pages(sources) = {
  parse-sources(sources, page-schema)
    .filter(source => not source.meta.draft)
    .sorted(key: source => (source.meta.order, source.id))
    .map(source => {
      let page-route = if source.meta.permalink == none {
        route(source.route-segments.map(slugify))
      } else {
        decode-url-path(source.meta.permalink)
      }
      (source: source, output: route-to-output(page-route))
    })
}
