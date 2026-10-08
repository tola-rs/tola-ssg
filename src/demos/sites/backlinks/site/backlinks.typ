#import "@tola/document:0.0.0": references
#let incoming-pages() = context {
  let incoming = references(to: auto, from-within: <body>)
  let documents = incoming.map(reference => reference.document).dedup(key: document => document.output)
  html.aside(aria-label: "Incoming pages")[
    #html.h2[Pages linking here]
    #if documents.len() == 0 [No incoming body links.]
    #html.ul[
      #for document in documents [
        #html.li[#link(document.location)[#document.route]]
      ]
    ]
  ]
}
