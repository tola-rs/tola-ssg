{{imports}}
#import "@tola/site:0.0.0": site
#import "seo.typ": head-entries

/// Write `404.html`, the page shown for an address the site does not publish.
/// -> content
#let not-found-template() = {
  document("404.html", format: "html", title: "Page not found")[
    #html.html(lang: site.language.tag)[
      #html.head[
        #for entry in head-entries("Page not found") { entry }
{{head}}
      ]
      #html.body[
        #html.elem("main")[
          #title()
          The page you requested could not be found.
        ]
      ]
    ]
  ]
}
