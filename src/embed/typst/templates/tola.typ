// Tola SSG base template (v__VERSION__)
//
// AUTO-GENERATED - Avoid modifying this file directly.
// Instead, extend it or create your own copy to reduce migration
// difficulty when upgrading to future versions with breaking changes.
//
// Handles math/table/figure rendering with proper HTML structure
// Provides page template with metadata for SSG

// ============================================================================
// Shared State
// ============================================================================

#let inside-figure = state("_tola-inside-figure", false)

// ============================================================================
// Base Template (Show Rules)
// ============================================================================

#let base(
  // CSS classes for customization
  figure-class: "",
  math-inline-class: "",
  math-block-class: "",
  // Math font (string or array for fallback)
  math-font: "New Computer Modern Math",
  body,
) = {
  show figure: it => context {
    if target() == "html" {
      inside-figure.update(true)
      html.figure(class: figure-class)[#it]
      inside-figure.update(false)
    } else { it }
  }

  // Note: No table show rule - Typst renders tables as native HTML <table>.
  // Using html.frame() on tables would convert them to SVG, causing internal
  // HTML elements (like html.code, html.span for math) to be ignored.

  show math.equation: set text(
    font: math-font,
    top-edge: "bounds",
    bottom-edge: "bounds",
  )

  show math.equation.where(block: false): it => context {
    if target() == "html" and not inside-figure.get() {
      html.span(class: math-inline-class, role: "math")[#html.frame(it)]
    } else { it }
  }

  show math.equation.where(block: true): it => context {
    if target() == "html" and not inside-figure.get() {
      html.div(class: math-block-class, role: "math")[#html.frame(it)]
    } else { it }
  }

  body
}

// ============================================================================
// Page Template
// ============================================================================

/// Page template with metadata for Tola SSG.
/// Usage: `page(title: "...", ...)[body]` or `page(title: "...", ..., head: [...])[body]`
#let page(
  // Content metadata (standard fields recognized by Tola SSG)
  title: none,
  summary: none,
  date: none,
  update: none,
  author: none,
  draft: false,
  tags: (),
  permalink: none,
  aliases: (),
  global-header: true,
  // Head content (optional)
  head: [],
  // Body content (required, positional)
  body,
  // Extra metadata fields (order, pinned, etc.)
  ..extra,
) = {
  [#metadata((
    title: title,
    summary: summary,
    date: date,
    update: update,
    author: author,
    draft: draft,
    tags: tags,
    permalink: permalink,
    aliases: aliases,
    global-header: global-header,
    ..extra.named(),
  )) <tola-meta>]

  show: base

  context {
    if target() == "html" {
      html.html[
        #html.head[#head]
        #html.body[#body]
      ]
    } else {
      body
    }
  }
}
