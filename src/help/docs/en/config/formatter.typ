#show: config(
  summary: [Configure Typst code formatting shared by `tola fmt` and `tola lsp`.],
)

Run `tola fmt --check` to verify formatting in CI and `tola fmt` to format in place. When invoked outside a Tola site, `tola fmt` formats `.typ` files in the current directory using these defaults.

#example(title: [Wrap lines at 88 columns])[
  ```toml
  [formatter]
  indent = 2
  print-width = 88
  prose-wrap = "none"
  ```
]

#key("indent")[
  Number of spaces per indentation level.
]

#key("print-width")[
  Target line width in columns. Values below `20` are clamped to `20`.
]

#key("prose-wrap")[
  How prose paragraphs wrap: `"none"` preserves existing line breaks, `"fill"` reflows to `print-width`, and `"sentence"` places each sentence on its own line.
]
