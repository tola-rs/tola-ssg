{{imports}}

/// The head entries one document carries: the character set, the viewport, the title, the
/// description, and the entries the selected features add for a page.
///
/// A document that is not a source, such as `404.html`, passes no `page` and carries only the
/// metadata entries.
///
/// - title (content | string): the document's title.
/// - description (none | content | string): the document's description.
/// - page (none | dictionary): the `(source:, output:)` record of the page being emitted.
/// -> array
#let head-entries(title, description: none, page: none) = {
  let entries = head-metadata(site, title: title, description: description){{entries}}
  entries
}{{outputs}}
