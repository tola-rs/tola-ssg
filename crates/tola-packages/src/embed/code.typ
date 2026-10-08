// @tola/code:0.0.0 - code themes, Typst's own highlighting, and the shared code stylesheet

/// The themes this package ships.
///
/// `code-themes.tokyo-night` is the theme file's path; pass it to `raw.theme` or
/// `render-code(dark-theme: …)`.
///
/// Example - check whether a theme name is a key:
///
/// ```typst
/// #import "@tola/code:0.0.0": code-themes
/// #assert("tokyo-night" in code-themes)
/// #assert("tokyo-night-dark" not in code-themes)
/// ```
///
/// Related: render-code
#import "@tola/host:0.0.0": code-themes

/// Render a raw element as code, keeping Typst's own highlighting.
///
/// Apply it with a show rule: `#show raw: render-code`. Typst prepares the element's lines and
/// highlighting before the show rule receives it. Put a code fence or `#raw(...)` in the document;
/// calling `render-code(raw(...))` directly skips that preparation and raises an error.
/// Outside HTML export the element is returned unchanged.
///
/// `raw.theme` decides each block's look before Typst highlights it: `auto` keeps Typst's default,
/// a path or theme bytes uses that theme, and `none` turns highlighting off. Set it where the block
/// is written, with `#set raw(theme: …)` or a `#show raw.where(…)` rule.
///
/// Load `code-stylesheet()` in the document's head so the browser applies the rendered token
/// styles. Set `raw.theme` for the primary appearance and `dark-theme` for the dark appearance.
///
/// Example - render code with light and dark themes in a complete page:
///
/// ```typst
/// #import "@tola/code:0.0.0": code-themes, code-stylesheet, render-code
/// #document("index.html", title: [Code])[
///   #html.elem("html", attrs: (lang: "en", "data-theme": "dark"))[
///     #html.head[
///       #html.title("Code")
///       #code-stylesheet()
///     ]
///     #html.body[
///       #title()
///       #set raw(theme: code-themes.github)
///       #show raw: render-code.with(dark-theme: code-themes.github-dark)
///       #raw("let answer = 42;", lang: "rust", block: true)
///     ]
///   ]
/// ]
/// ```
///
/// Related: code-themes, code-stylesheet
///
/// - raw (content): the raw element to render.
/// - dark-theme (none | path | bytes): the theme for the site's dark state — a `code-themes` value,
///   a `path("/…")` value, or theme bytes. It applies while the site's root element has
///   `data-theme="dark"`. The file is read during the build, so a missing theme fails the build; a
///   plain string is refused, because it would resolve inside this package.
/// - attrs (dictionary): HTML attributes for the element Tola renders code into — `class`, `id`,
///   `data-*`. `class` and `style` extend the helper's own; `data-lang` is refused, since the
///   language tag sets it.
/// -> content
#let render-code = {
  import "@tola/host:0.0.0": render-code as render

  // The carrier element whose `theme` field loads the dark theme. The parameter below shadows the
  // built-in `raw` constructor, so it is captured here.
  let code-element = raw

  (raw, dark-theme: none, attrs: (:)) => context {
    if target() != "html" {
      return raw
    }
    if not raw.has("lines") {
      panic("render-code needs a code block from the document; apply it with a show rule")
    }
    let dark = if dark-theme == none {
      none
    } else if type(dark-theme) == str {
      panic("`dark-theme` must be a theme path or bytes; write `path(\"/…\")` or pass a value from `code-themes`")
    } else {
      code-element("", theme: dark-theme)
    }
    render(raw, dark, attrs)
  }
}

/// The browser URL the code stylesheet is mounted at.
///
/// The stylesheet colors the code `render-code` renders. Use this URL when the head entry needs
/// attributes of its own; `code-stylesheet` returns the ready-made entry. Every build publishes
/// the shared stylesheet under `_tola/`, and this URL includes `site.base-path`.
///
/// Example - give the head entry attributes of its own:
///
/// ```typst
/// #import "@tola/code:0.0.0": code-stylesheet-url
/// #let entry = html.link(rel: "stylesheet", href: code-stylesheet-url(), media: "screen")
/// #document("index.html")[
///   #html.html[
///     #html.head(entry)
///     #html.body[Code]
///   ]
/// ]
/// ```
///
/// Related: code-stylesheet
#import "@tola/host:0.0.0": code-stylesheet-url

/// The head entry that loads the code stylesheet.
///
/// Put it directly in a head. When the entry needs attributes of its own, build it from
/// `code-stylesheet-url()` instead.
///
/// Example - load the stylesheet from a document head:
///
/// ```typst
/// #import "@tola/code:0.0.0": code-stylesheet
/// #document("index.html")[
///   #html.html[
///     #html.head(code-stylesheet())
///     #html.body[Code]
///   ]
/// ]
/// ```
///
/// Related: code-stylesheet-url, render-code
///
/// -> content
#let code-stylesheet() = html.link(
  rel: "stylesheet",
  href: code-stylesheet-url(),
)
