#import "@tola/address:0.0.0": output-to-url
#let navigation(pages) = html.nav(aria-label: "Selected pages")[
  #html.ul[
    #for selected in pages [
      #html.li[#link(output-to-url(selected.output))[#selected.source.meta.title]]
    ]
  ]
]
