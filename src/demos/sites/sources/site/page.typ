#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": asset-url, output-to-url
#import "@tola/web:0.0.0": head-metadata

#let page(output, page-title, body) = document(output, format: "html", title: page-title)[
  #html.html(lang: site.language.tag)[
    #html.head[
      #for entry in head-metadata(site, title: page-title) { entry }
      #html.link(rel: "stylesheet", href: asset-url("/assets/site.css"))
    ]
    #html.body[
      #html.nav(aria-label: "Primary")[#link(output-to-url("index.html"))[Demo home]]
      #html.main[#body]
    ]
  ]
]
