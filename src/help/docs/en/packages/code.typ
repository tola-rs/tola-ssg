#show: package(
  summary: [Syntax-highlight code blocks in HTML export with light and dark themes.],
)

#export(
  "code-syntaxes",
  summary: [Dictionary of extra syntax definitions bundled with Tola, keyed by language tag.],
  related: ("code-language", "render-code"),
)[
  If `raw.syntaxes` defines a syntax with the same tag, your site's definition takes precedence.
]

#export(
  "code-language",
  summary: [Normalize a language alias to the syntax name recognized by Typst's `raw(..., lang: ...)`.],
  params: (
    language: [Source language tag or alias (for example `"sbcl"` → `"lisp"`).],
  ),
  related: ("code-syntaxes", "render-code"),
)

#export(
  "code-themes",
  summary: [Dictionary of bundled `.tmTheme` file paths for `raw.theme` and `render-code(dark-theme: ...)`.],
  related: ("render-code", "code-stylesheet"),
)[
  Pass any entry directly—such as `code-themes.github` or `code-themes.tokyo-night`—to `#set raw(theme: ...)` or `render-code(dark-theme: ...)`.
]

#export(
  "render-code",
  summary: [Render Typst-highlighted `raw` elements as styled HTML with optional dark-theme tokens.],
  params: (
    raw: [The `raw` element passed by a `#show raw:` rule.],
    dark-theme: [Theme (`code-themes.*`, `path`, or `bytes`) active when the root HTML element has `data-theme="dark"`. Omit to use the light theme in both modes.],
    attrs: [Extra HTML attributes merged onto the code element (for example `(class: "example",)`). `class` and `style` append; `data-lang` is set from the code language.],
  ),
  related: ("code-themes", "code-stylesheet", "code-stylesheet-url"),
)[
  Apply as `#show raw: render-code` (calling `render-code(raw(...))` directly fails because Typst has not highlighted the element yet). In non-HTML exports, `render-code` returns the original `raw` element unchanged.

  Configure the light theme with `#set raw(theme: ...)` (`auto` uses Typst's default theme; `none` disables both light and dark highlighting). Include `code-stylesheet()` inside `<head>` so token colors load in the browser.

  #example(title: [Highlight HTML code with light and dark themes], check: "default")[
  ```typst
  #import "@tola/code:0.0.0": code-stylesheet, code-themes, render-code

  #document("index.html", title: [Code])[
    #html.elem("html", attrs: (lang: "en", "data-theme": "dark"))[
      #html.head[
        #html.title("Code")
        #code-stylesheet()
      ]
      #html.body[
        #set raw(theme: code-themes.github)
        #show raw: render-code.with(dark-theme: code-themes.github-dark)
        #raw("let answer = 42;", lang: "rust", block: true)
      ]
    ]
  ]
  ```
  ]
]

#export(
  "code-stylesheet-url",
  summary: [Return the URL of the shared syntax-highlighting stylesheet, including `site.base-path`.],
  related: ("code-stylesheet", "render-code"),
)[
  Use this when you want to construct a custom `html.link(...)` element with extra attributes such as `media`. The stylesheet is published automatically whenever referenced.
]

#export(
  "code-stylesheet",
  summary: [Return the `<link rel="stylesheet">` element loading Tola's syntax-highlighting CSS for `<head>`.],
  related: ("code-stylesheet-url", "render-code"),
)
