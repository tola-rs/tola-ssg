// @tola/code:0.0.0 - code themes, Typst's own highlighting, and the shared code stylesheet

/// The themes this package ships.
///
/// `code-themes.tokyo-night` is the theme file's path; pass it to `raw.theme` or
/// `render-code(dark-theme: …)`.
///
///
/// ```typst
/// #import "@tola/code:0.0.0": code-themes
/// #assert("tokyo-night" in code-themes)
/// #assert("tokyo-night-dark" not in code-themes)
/// ```
///
/// Related: render-code
#import "@tola/host:0.0.0": code-themes

/// Additional syntax definitions keyed by language.
///
/// Tola uses these definitions when rendering code in a site. A matching syntax supplied
/// through `raw.syntaxes` takes precedence. Use this dictionary to access the syntax files.
#import "@tola/host:0.0.0": code-syntaxes

/// Select a built-in Typst syntax for a source-code label.
///
/// Use this when constructing `raw(..., lang: ...)`. For example, `sbcl` and `ccl`
/// select `lisp`, and `babashka` selects `clj`. Other labels pass through as written.
/// In a literal fence, write the returned language name.
///
///
/// Replace `site.typ` in a new site with this example. Run `tola build` from the site root.
/// The code block in `public/index.html` uses Lisp syntax.
///
/// ```typst
/// #import "@tola/code:0.0.0": code-language
/// #document("index.html")[
///   #raw("(defun twice (x) (* x 2))", lang: code-language("sbcl"))
/// ]
/// ```
#import "@tola/host:0.0.0": code-language

/// Render a raw element as code, keeping Typst's own highlighting.
///
/// Apply it with a show rule: `#show raw: render-code`. Typst prepares the element's lines and
/// highlighting before the show rule receives it. Put a code fence or `#raw(...)` in the document;
/// calling `render-code(raw(...))` directly skips that preparation and raises an error.
/// Outside HTML export the element is returned unchanged.
///
/// `raw.theme` decides each block's look before Typst highlights it: `auto` keeps Typst's default,
/// a path or theme bytes uses that theme, and `none` turns off highlighting in both light and dark themes. Set it where the block
/// is written, with `#set raw(theme: …)` or a `#show raw.where(…)` rule.
///
/// Load `code-stylesheet()` in the document's head so the browser applies the rendered token
/// styles. Set `raw.theme` for the primary appearance and `dark-theme` for the dark appearance.
///
///
/// Save this as `site.typ` in a new site. Run `tola dev` and open the printed URL.
/// The page shows a highlighted Rust block in the dark theme. Set `data-theme` to `"light"`
/// to see the light theme.
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
/// - dark-theme (none | path | bytes): Theme used when the root element has `data-theme="dark"`. If omitted,
///   use the main highlighting in both modes. Pass a value from `code-themes`, a Typst `path`,
///   or theme bytes. Tola reads the theme at build time.
/// - attrs (dictionary): Extra HTML attributes, for example `(class: "example",)`. If omitted,
///   use the generated attributes. `class` and `style` append to existing values. The code's
///   language sets `data-lang`, so leave that key out of `attrs`.
/// -> content
#let render-code = {
  import "@tola/host:0.0.0": render-code as render

  // The element whose `theme` field loads the dark theme: the `raw` constructor, captured before
  // the parameter below shadows it.
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

/// Return the browser URL for the shared code stylesheet.
///
/// The stylesheet colors the code `render-code` renders. Use this URL when the head entry needs
/// attributes of its own; `code-stylesheet` returns the ready-made entry. Every build publishes
/// the shared stylesheet under `_tola/`, and this URL includes `site.base-path`.
///
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
