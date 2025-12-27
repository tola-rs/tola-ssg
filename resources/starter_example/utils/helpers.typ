// Shared utilities
// Import: #import "/utils/helpers.typ": *

#let hr = html.hr(class: "border-surface my-8")

#let nav-link(href, label) = html.a(class: "text-muted hover:text-accent transition-colors", href: href)[#label]
#let tag(name) = html.span(class: "px-2 py-1 text-xs bg-surface rounded text-accent")[#name]
#let card(title: none, body) = html.div(class: "p-4 bg-surface rounded-lg")[
  #if title != none { html.h3(class: "font-semibold text-accent mb-2")[#title] }
  #body
]
#let flex-row(gap: "4", ..items) = html.div(class: "flex gap-" + gap)[#for item in items.pos() { item }]
#let grid(cols: "2", gap: "4", ..items) = html.div(
  class: "grid grid-cols-" + cols + " gap-" + gap,
)[#for item in items.pos() { item }]

#let to-string(value) = repr(value)
