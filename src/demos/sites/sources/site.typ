#import "@tola/source:0.0.0": all-sources
#import "site/selection.typ": select-pages
#import "site/navigation.typ": navigation
#import "site/page.typ": page
#import "site/not-found.typ": not-found

#let sources = all-sources()
#let pages = select-pages(sources)
#asset("source-metadata.json", bytes(json.encode(sources.map(source => (
  path: source.path, title: source.meta.title,
)))))
#for selected in pages {
  page(selected.output, selected.source.meta.title)[
    #navigation(pages)
    #include selected.source.file
  ]
}
#not-found()
