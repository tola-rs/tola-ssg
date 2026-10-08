#import "@tola/document:0.0.0": current-document, headings
#let table-of-contents() = context {
  let document = current-document()
  html.nav(aria-label: "On this page")[
    #html.p[Contents for #document.route]
    #html.ul[
      #for section in headings(depth: 2).filter(section => section.outlined) [
        #html.li[#link(section.location, section.text)]
      ]
    ]
  ]
}
