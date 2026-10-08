#import "@tola/source:0.0.0": all-sources
#import "site/page.typ": page
#import "site/not-found.typ": not-found
#let source = all-sources().find(source => source.path == "index.typ")
#page("index.html", source.meta.title)[#include source.file]
#not-found()
