// Tola SSG base template:
//
// Handles math/table/figure rendering with proper HTML structure
// Provides page template with metadata for SSG

// ============================================================================
// Base Template (Show Rules)
// ============================================================================

#let base(body) = {
  let _tola-svg-inside-figure = state("_tola-svg-inside-figure", false)

  show figure: it => {
    _tola-svg-inside-figure.update(true)
    it
    _tola-svg-inside-figure.update(false)
  }

  show table: it => context {
    if not _tola-svg-inside-figure.get() {
      html.div(class: "tola-table")[#html.frame(it)]
    } else { it }
  }

  show math.equation.where(block: false): it => context {
    if not _tola-svg-inside-figure.get() {
      html.span(class: "tola-inline-math", role: "math")[#html.frame(it)]
    } else { it }
  }

  show math.equation.where(block: true): it => context {
    if not _tola-svg-inside-figure.get() {
      html.div(class: "tola-block-math", role: "math")[#html.frame(it)]
    } else { it }
  }

  body
}

// ============================================================================
// Page Template
// ============================================================================

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

  body,
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
  )) <tola-meta>]

  show: base

  body
}
