// Base template with show rules for automatic Tailwind styling
// Import: #import "/templates/base.typ": post, page

#import "/utils/helpers.typ": *

// ============================================================================
// Configuration
// ============================================================================

#let site = (name: "My Blog", author: "Your Name")

#let colors = (
  accent: "text-cyan-400",
  code: "text-purple-300",
  muted: "text-slate-400",
)

#let hr = html.hr(class: "border-surface my-8")

// ============================================================================
// Post Template
// ============================================================================

#let post(
  title: none,
  date: none,
  update: none,
  author: none,
  summary: none,
  tags: (),
  draft: false,
  body,
) = {
  // Metadata for tola
  [#metadata((
    title: title,
    date: date,
    update: update,
    author: author,
    summary: summary,
    tags: tags,
    draft: draft,
  )) <tola-meta>]

  // --------------------------------------------------------------------------
  // Views
  // --------------------------------------------------------------------------

  let title-view = if title != none {
    html.h1(class: "text-3xl sm:text-4xl font-bold text-center my-6")[#title]
  }

  let subtitle-view = if date != none or author != none {
    let parts = ()
    if date != none { parts.push(date) }
    if author != none { parts.push("by " + author) }
    html.div(class: "text-center " + colors.muted)[#parts.join(" · ")]
  }

  let summary-view = if summary != none {
    html.div(class: "text-center italic my-4 " + colors.muted)[#summary]
  }

  let tags-view = if tags.len() > 0 {
    html.div(class: "flex flex-wrap justify-center gap-2 my-4")[
      #for tag in tags { html.span(class: "px-2 py-1 text-sm bg-surface rounded " + colors.accent)[#tag] }
    ]
  }

  // --------------------------------------------------------------------------
  // Show Rules: Lists
  // --------------------------------------------------------------------------

  show list: it => html.ul(class: "list-disc ml-6 my-4 space-y-1")[
    #for item in it.children { html.li[#item.body] }
  ]
  show enum: it => html.ol(class: "list-decimal ml-6 my-4 space-y-1")[
    #for item in it.children { html.li[#item.body] }
  ]

  // --------------------------------------------------------------------------
  // Show Rules: Headings
  // --------------------------------------------------------------------------

  show heading.where(level: 1): it => {
    let id = lower(repr(it.body).replace("\"", "").replace(" ", "-"))
    html.h2(class: "text-2xl font-bold mt-8 mb-4 " + colors.accent, id: id)[
      #html.a(class: "hover:underline underline-offset-4", href: "#" + id)[#it.body]
    ]
  }
  show heading.where(level: 2): it => {
    let id = lower(repr(it.body).replace("\"", "").replace(" ", "-"))
    html.h3(class: "text-xl font-semibold mt-6 mb-3", id: id)[
      #html.a(class: "hover:underline underline-offset-4", href: "#" + id)[#it.body]
    ]
  }

  // --------------------------------------------------------------------------
  // Show Rules: Code
  // --------------------------------------------------------------------------

  show raw.where(block: false): it => html.code(class: "font-semibold " + colors.code)[#it.text]
  show raw.where(block: true): it => {
    let lang = if it.lang != none { "language-" + it.lang } else { "" }
    html.pre(class: "bg-surface rounded-lg p-4 my-4 overflow-x-auto border border-white/10")[
      #html.code(class: lang + " text-sm")[#it.text]
    ]
  }

  // --------------------------------------------------------------------------
  // Show Rules: Text Elements
  // --------------------------------------------------------------------------

  show quote: it => html.blockquote(class: "border-l-4 border-accent pl-4 my-4 italic " + colors.muted)[#it.body]
  show link: it => html.a(
    class: "underline underline-offset-4 hover:" + colors.accent,
    href: repr(it.dest).replace("\"", ""),
  )[#it.body]
  show strike: it => html.del[#it.body]
  show image: html.frame

  // --------------------------------------------------------------------------
  // Show Rules: Math
  // --------------------------------------------------------------------------

  let inside-figure = state("inside-figure", false)

  show figure: it => {
    inside-figure.update(true)
    html.figure(class: "my-6 mx-auto w-fit")[#it]
    inside-figure.update(false)
  }
  show math.equation.where(block: false): it => context {
    if not inside-figure.get() { html.span(class: "inline-flex align-middle", role: "math")[#html.frame(it)] } else {
      it
    }
  }
  show math.equation.where(block: true): it => context {
    if not inside-figure.get() { html.figure(class: "my-6 flex justify-center", role: "math")[#html.frame(it)] } else {
      it
    }
  }

  // --------------------------------------------------------------------------
  // Render
  // --------------------------------------------------------------------------

  html.main(class: "max-w-3xl mx-auto px-4 py-8")[
    #html.article(class: "space-y-4")[
      #title-view #subtitle-view #tags-view #summary-view #hr #body
    ]
  ]
}

// ============================================================================
// Page Template (for non-post pages like index)
// ============================================================================

#let page(title: none, body) = {
  if title != none { [#metadata((title: title)) <tola-meta>] }

  show list: it => html.ul(class: "list-disc ml-6 space-y-1")[#for item in it.children { html.li[#item.body] }]
  show enum: it => html.ol(class: "list-decimal ml-6 space-y-1")[#for item in it.children { html.li[#item.body] }]
  show heading.where(level: 1): it => html.h2(class: "text-2xl font-bold mt-8 mb-4 " + colors.accent)[#it.body]
  show heading.where(level: 2): it => html.h3(class: "text-xl font-semibold mt-6 mb-3")[#it.body]
  show raw.where(block: false): it => html.code(class: "font-semibold " + colors.code)[#it.text]
  show quote: it => html.blockquote(class: "border-l-4 border-accent pl-4 italic " + colors.muted)[#it.body]
  show link: it => html.a(
    class: "underline underline-offset-4 hover:" + colors.accent,
    href: repr(it.dest).replace("\"", ""),
  )[#it.body]

  html.main(class: "max-w-3xl mx-auto px-4 py-8")[#body]
}
