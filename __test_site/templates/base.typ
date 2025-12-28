#let base(body) = {
  html.elem("html", attrs: (lang: "en"))[
    #html.elem("head")[
      #html.elem("title")[Test]
    ]
    #html.elem("body")[
      #body
    ]
  ]
}
