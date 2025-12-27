#import "/templates/base.typ": page

#show: page.with(title: "Welcome")

= Welcome to My Blog

Built with #link("https://typst.app")[Typst] and #link("https://github.com/tola-ssg/tola-ssg")[Tola].

== Getting Started

- Edit this file at `content/index.typ`
- Add new posts in `content/posts/`
- Customize styles in `assets/styles/tailwind.css`
- Run `tola serve` for live preview

== Features

+ *Fast Rebuilds* — Only changed files are recompiled
+ *Live Reload* — See changes instantly in your browser
+ *Tailwind CSS* — Utility-first styling out of the box
+ *Math Support* — Beautiful equations with Typst

== Math Demo

Inline math: $e^(i pi) + 1 = 0$

Block math:

$ integral_0^infinity e^(-x^2) d x = sqrt(pi) / 2 $

#quote[
  Typst is a new markup-based typesetting system that is designed to be as powerful as LaTeX while being much easier to learn and use.
]
