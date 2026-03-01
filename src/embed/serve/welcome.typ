// Welcome page for tola serve (no content yet)
// CSS is injected via with_style() in build.rs

#html.style(sys.inputs.at("css", default: "")) // welcome.css

#html.div(class: "container")[
  #html.div(class: "logo")[#image("tola.webp", width: 120pt)]
  #html.p(class: "tagline")[Static site generator for Typst-based blogs]

  #html.div(class: "card")[
    == Getting Started

    + Create content in `content/` directory
    + Add your `.typ` files
    + Run `tola build` to generate your site
    + Your site will appear here automatically!
  ]

  #html.div(class: "card")[
    == Quick Tips

    - Use `tola serve` for live reload
    - Customize templates in `templates/` directory
    - Add assets to `assets/` directory
  ]

  #html.div(class: "links")[
    #link("https://github.com/tola-rs/tola-ssg")[GitHub]
    #link("https://github.com/tola-rs/tola-ssg")[Documentation]
  ]

  #html.footer[Powered by tola v__VERSION__]
]
