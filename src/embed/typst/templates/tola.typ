// Tola SSG base template (v__VERSION__)
//
// AUTO-GENERATED - Avoid modifying this file directly.
// Instead, extend it or create your own copy to reduce migration
// difficulty when upgrading to future versions with breaking changes.
//
// Handles math/table/figure rendering with proper HTML structure
// Provides page template with metadata for SSG

// ============================================================================
// Format Detection: is-html vs target()
// ============================================================================
//
// Two ways to detect HTML output, each with different use cases:
//
// 1. is-html (sys.inputs.format == "html")
//    - Static value injected by Tola at compile time
//    - Works during scan phase (Eval-only, no Layout)
//    - Use for: image show rules (need to extract src paths during scan)
//    - Caveat: Still "html" even inside html.frame() internal rendering
//
// 2. context { target() }
//    - Runtime check, returns "html" or "paged"
//    - Returns "paged" inside html.frame() (when rendering math to SVG)
//    - Use for: math show rules with html.frame() (avoids "paged export" warnings)
//    - Caveat: Requires context block, not evaluated during scan phase
//
// For typst CLI users: add --input format=html when compiling to HTML.
#let is-html = sys.inputs.at("format", default: none) == "html"

// ============================================================================
// Shared State
// ============================================================================

#let inside-figure = state("_tola-inside-figure", false)

// ============================================================================
// Base Template (Show Rules)
// ============================================================================

#let tola-base(
  // CSS classes for customization
  figure-class: "",
  math-inline-class: "",
  math-block-class: "",
  // Math font (string or array for fallback)
  math-font: "New Computer Modern Math",
  body,
) = {
  show figure: it => {
    if is-html {
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

  // Math equations: use target() instead of is-html
  // - html.frame() internally renders to SVG using "paged" mode
  // - If we used is-html, the show rule would try to wrap again, causing warnings
  // - target() returns "paged" inside html.frame(), so the show rule skips
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
// Date Utilities
// ============================================================================

/// Parse date string to datetime (simple version).
#let _parse-date(s) = {
  if s == none { return none }
  if type(s) == datetime { return s }
  let s = str(s).split("T").at(0)
  let parts = s.split("-")
  if parts.len() != 3 { return none }
  datetime(year: int(parts.at(0)), month: int(parts.at(1)), day: int(parts.at(2)))
}

// ============================================================================
// Page Template
// ============================================================================

/// Page template with metadata for Tola SSG.
/// Usage: `tola-page(title: "...", ...)[body]` or `tola-page(title: "...", ..., head: [...])[body]`
///
/// Date fields (date, update) are automatically converted from string to datetime.
#let tola-page(
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
  // Auto-convert date strings to datetime
  let date = _parse-date(date)
  let update = _parse-date(update)

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

  show: tola-base

  if is-html {
    html.html[
      #html.head[#head]
      #html.body[#body]
    ]
  } else {
    body
  }
}

// ============================================================================
// Template Builder
// ============================================================================

/// Create a custom template with automatic parameter forwarding.
///
/// This helper reduces boilerplate when creating templates that extend tola-page.
/// It automatically handles:
/// - Parameter declaration and forwarding to tola-page
/// - Applying base show rules (won't forget `show: base`)
/// - Head content generation from metadata
///
/// Parameters:
/// - `base`: Show rule function to apply (e.g., your custom base with heading styles)
/// - `head`: Function `(meta) => content` to generate <head> content (e.g., og-tags)
/// - `view`: Function `(body, meta) => content` to wrap the body with layout
/// - `transform-meta`: Function `(meta) => meta` to transform metadata before passing to tola-page.
///   Use this to derive fields from source path (e.g., extract date/permalink from filename).
///
/// Example:
/// ```typst
/// #import "/templates/tola.typ": wrap-page
/// #import "/templates/base.typ": base
/// #import "/utils/tola.typ": og-tags
///
/// #let post = wrap-page(
///   base: base,
///   head: (m) => og-tags(title: m.title, published: m.date),
///   view: (body, m) => {
///     show heading.where(level: 1): it => html.h2[#it.body]
///     html.article[
///       #if m.title != none { html.h1[#m.title] }
///       #body
///     ]
///   },
/// )
/// ```
#let wrap-page(
  base: none,
  head: none,
  view: (body, meta) => body,
  transform-meta: none,
) = (body, ..args) => {
  let meta = args.named()

  // Transform meta first (e.g., derive date/permalink from source)
  if transform-meta != none { meta = transform-meta(meta) }

  // Auto-convert date strings to datetime (after transform, so derived dates get converted)
  if "date" in meta { meta.date = _parse-date(meta.date) }
  if "update" in meta { meta.update = _parse-date(meta.update) }

  let head-content = if head != none { head(meta) }
  let base-fn = if base == none { it => it } else { base }

  tola-page(..meta, head: head-content)[
    #show: base-fn
    #view(body, meta)
  ]
}
