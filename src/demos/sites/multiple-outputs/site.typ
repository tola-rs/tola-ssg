#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": output-to-url
#import "site/page.typ": page
#import "site/chapters.typ": chapter-list, chapter-table
#import "site/not-found.typ": not-found
#let source = all-sources().find(source => source.path == "index.typ")
#let chapters = json("static/data/chapters.json")
#page("index.html", source.meta.title)[
  #include source.file
  #chapter-list(chapters)
  #link(output-to-url("table/index.html"))[See the table view]
  #link(output-to-url("chapters.json"))[Download the generated data]
]
#page("table/index.html", "The same chapters as a table")[
  #include source.file
  #chapter-table(chapters)
]
#asset("chapters.json", bytes(json.encode(chapters.map(chapter => (
  id: chapter.id, title: chapter.title,
)))))
#not-found()
