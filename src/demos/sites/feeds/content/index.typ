#import "@tola/source:0.0.0": tola-meta
#import "@tola/address:0.0.0": asset-url
#tola-meta((
  title: "A field note",
  published: datetime(year: 2026, month: 9, day: 1),
  feed-summary: [A *short* summary.],
  feed-content: [Portable *body* with a #link("https://typst.app/")[source].],
))
#html.article(id: "entry-body")[
  #title()
  = Inside the article
  The complete article includes this paragraph and image.
  #html.img(src: asset-url("/assets/stripes.png"), width: 128, height: 64, alt: "Three colored bands")
]
