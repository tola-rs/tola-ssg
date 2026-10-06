{{imports}}
#import "@tola/code:0.0.0": render-code, code-stylesheet, code-themes
#import "@tola/site:0.0.0": site
#import "seo.typ": head-entries

/// Write one page to its output file as a complete HTML document.
///
/// - page (dictionary): one `(source:, output:)` record from `select-pages`.
/// -> content
#let page-template(page) = {
  let meta = page.source.meta
  let authors = if meta.authors == none {
    ()
  } else {
    meta.authors.map(author => if type(author) == dictionary { author.name } else { author })
  }
  document(page.output, format: "html", title: meta.title, description: meta.description, author: authors, keywords: meta.tags)[
    #set raw(theme: code-themes.github)
    #show raw: render-code.with(dark-theme: code-themes.github-dark)
    #html.html(lang: site.language.tag)[
      #html.head[
        #for entry in head-entries(meta.title, description: meta.description, page: page) { entry }
        #code-stylesheet()
{{head}}
      ]
      #html.body[
        #html.elem("main")[
          #include page.source.file
        ] <page>
      ]
    ]
  ]
}
