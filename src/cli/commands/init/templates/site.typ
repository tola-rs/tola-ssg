// Publish every source below `content/` at the route its file layout gives it.
#import "@tola/address:0.0.0": join-route, output-of
#import "@tola/site:0.0.0": site
#import "@tola/source:0.0.0": all-sources

#set text(
  lang: site.language.lang,
  script: site.language.script,
  region: site.language.region,
)

#for source in all-sources() {
  document(output-of(join-route(source.route-segments)), format: "html")[
    #include source.file
  ]
}

#document("404.html", format: "html", title: "Page not found")[
  The page you requested could not be found.
]
