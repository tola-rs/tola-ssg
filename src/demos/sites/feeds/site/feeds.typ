#import "@tola/web:0.0.0": feed
#let publish-feeds(source, output) = {
  let entry = (
    id: "urn:tola-demo:field-note",
    target: output,
    published: source.meta.published,
    summary: source.meta.feed-summary,
  )
  feed(output: "summary.xml", entries: (entry,))
  feed(output: "portable.xml", entries: ((..entry, content: source.meta.feed-content),))
  feed(output: "whole.xml", entries: ((..entry, content: (document: output)),))
  feed(output: "selected.xml", entries: ((..entry, content: (document: output, id: "entry-body")),))
}
