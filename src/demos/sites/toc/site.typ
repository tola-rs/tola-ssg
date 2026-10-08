#import "@tola/source:0.0.0": all-sources
#import "site/page.typ": page
#import "site/toc.typ": table-of-contents
#import "site/not-found.typ": not-found

#let sources = all-sources()
#let guide = sources.find(source => source.path == "index.typ")
#let other = sources.find(source => source.path == "other.typ")
#page("index.html", guide.meta.title)[
  #table-of-contents()
  #include guide.file
]
#page("other/index.html", other.meta.title)[
  #table-of-contents()
  #include other.file
]
#not-found()
