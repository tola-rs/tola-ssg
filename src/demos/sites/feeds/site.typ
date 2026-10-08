#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": output-to-url
#import "site/page.typ": page
#import "site/feeds.typ": publish-feeds
#import "site/not-found.typ": not-found
#let source = all-sources().find(source => source.path == "index.typ")
#page("index.html", source.meta.title)[
  #html.nav(aria-label: "Feed choices")[
    #for (output, name) in ("summary.xml": "Summary", "portable.xml": "Portable text", "whole.xml": "Whole body", "selected.xml": "Selected body") [
      #link(output-to-url(output))[#name]     ]
  ]
  #include source.file
  #html.aside[Outside the article: this sidebar belongs only to the whole-body feed.]
]
#publish-feeds(source, "index.html")
#not-found()
