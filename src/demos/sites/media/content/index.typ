#import "@tola/source:0.0.0": tola-meta
#import "@tola/address:0.0.0": asset-url
#import "@tola/icon:0.0.0": icon, icon-url
#import "@tola/image:0.0.0": image-metadata, resize-image
#tola-meta((title: "Local icons and resized images"))
#title()

= Inline icons
#html.p(style: "color: #2355a5")[
  #icon("demo:leaf", label: "Leaf") #icon("demo:sun", label: "Sun")
  The leaf inherits this color; the sun keeps its fixed fill.
]
#html.img(src: icon-url("demo:leaf"), width: 24, height: 24, alt: "Leaf published as a separate image")

= Input files and browser URLs
#let input = path("/static/image-input/stripes.png")
#let metadata = image-metadata(input)
#let resized = resize-image(input, width: 48, op: "fit-width", format: "png")
#html.p[The input is #metadata.width by #metadata.height pixels; the derivative is #resized.width by #resized.height.]
#html.img(src: resized.url, width: resized.width, height: resized.height, alt: "Three colored bands, resized")
#html.img(src: asset-url("/assets/stripes.png"), width: 128, height: 64, alt: "The separately published original")

The image input under `static/image-input/` is private: resizing publishes its derivative, not that file.
The copy under the asset tree is published because the configuration declares that tree.
