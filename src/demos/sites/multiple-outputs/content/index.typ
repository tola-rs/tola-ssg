#import "@tola/source:0.0.0": tola-meta, current-source
#import "@tola/document:0.0.0": current-document
#let source = current-source()
#tola-meta((title: "One source, several outputs"))
#title()
#html.p[Source: #source.path; document: #context current-document().output]
One JSON input supplies a list page, a table page, and a generated JSON download.
Reading an input does not publish it; the asset declaration publishes the selected fields.
