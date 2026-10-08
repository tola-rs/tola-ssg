#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": output-to-url
#import "site/page.typ": page
#import "site/backlinks.typ": incoming-pages
#import "site/not-found.typ": not-found

#let sources = all-sources()
#let source-page(source-path, output) = {
  let source = sources.find(source => source.path == source-path)
  page(output, source.meta.title)[
    #html.nav(aria-label: "All pages")[
      #link(output-to-url("a/index.html"))[Alpha] / #link(output-to-url("b/index.html"))[Beta] /
      #link(output-to-url("topic/index.html"))[Topic]
    ]
    #html.article[#include source.file] <body>
    #incoming-pages()
  ]
}
#source-page("index.typ", "index.html")
#source-page("a.typ", "a/index.html")
#source-page("b.typ", "b/index.html")
#source-page("topic.typ", "topic/index.html") <topic-page>
#not-found()
