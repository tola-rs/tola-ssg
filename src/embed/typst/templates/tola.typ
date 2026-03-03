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

// Inline math baseline fix (pin + measure).
#let bounded(eq) = text(top-edge: "bounds", bottom-edge: "bounds", eq)
#let equations-height-dict = state("eq_height_dict", (:))
#let is-inside-pin = state("inside_pin", false)

#let pin(label) = context {
  let height = here().position().y
  equations-height-dict.update(dict => {
    if label in dict.keys() or height < 0.000001pt {
      dict
    } else {
      dict.insert(label, height)
      dict
    }
  })
}

#let add-pin(eq) = {
  let label = repr(eq)
  is-inside-pin.update(true)
  $ inline(pin(label)#bounded(eq)) $
  is-inside-pin.update(false)
}

#let to-em(pt) = str(pt / text.size.pt()) + "em"

#let math-span(class: "", style: none, body) = {
  let attrs = (role: "math")
  if class != "" {
    attrs.insert("class", class)
  }
  if style != none {
    attrs.insert("style", style)
  }
  html.elem("span", body, attrs: attrs)
}

#let render-inline-math(eq, class: "") = context {
  if is-inside-pin.get() {
    return math-span(class: class)[#html.frame(bounded(eq))]
  }

  let label = repr(eq)
  let cache = equations-height-dict.final()
  if label in cache.keys() {
    let reference-height = cache.at(label, default: none)
    equations-height-dict.update(dict => {
      dict.insert(label, reference-height)
      dict
    })

    let measured-height = measure(bounded(eq)).height
    let shift = measured-height - reference-height
    let style = "vertical-align: -" + to-em(shift.pt()) + ";"
    math-span(class: class, style: style)[#html.frame(bounded(eq))]
  } else {
    math-span(class: class)[#box(html.frame(add-pin(eq)))]
  }
}

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
  // Figure wrapper: use target() to avoid html.elem warnings inside html.frame()
  // internal paged render passes.
  show figure: it => context {
    if target() == "html" {
      inside-figure.update(true)
      let wrapped = html.figure(class: figure-class)[#it]
      inside-figure.update(false)
      wrapped
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

  // Math equations: use target()
  // - html.frame() internally renders to SVG using "paged" mode
  // - target() returns "paged" inside html.frame(), so the show rule skips
  show math.equation.where(block: false): it => context {
    if target() == "html" and not inside-figure.get() {
      render-inline-math(it, class: math-inline-class)
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

/// Parse date string to datetime (strict version).
#let _parse-date(s) = {
  if s == none { return none }
  if type(s) == datetime { return s }
  let s = str(s).split("T").at(0)
  let parts = s.split("-")
  assert(parts.len() == 3, message: "Invalid date format: '" + s + "', expected YYYY-MM-DD")
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

  // Keep this top-level branch outside context so query/validate sees body directly.
  let is-html = sys.inputs.at("format", default: "paged") == "html"
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
