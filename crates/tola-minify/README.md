# tola-minify

Deterministic minification of CSS and JavaScript source text: the same input, the same output.

```rust
use std::path::Path;
use tola_minify::{MinifiedLanguages, MinifyRequest};

let languages = MinifiedLanguages::new(true, false);
let stylesheet = MinifyRequest::for_source(Path::new("assets/app.css"), languages).unwrap();
assert_eq!(stylesheet.minify(".a { color: red; } .b { color: red; }").unwrap(), ".a,.b{color:red}");

// A source already named `*.min.*`, or a language you leave off, is not minified.
assert!(MinifyRequest::for_source(Path::new("assets/app.min.css"), languages).is_none());
assert!(MinifyRequest::for_source(Path::new("assets/app.js"), languages).is_none());
```

`MinifiedLanguages` selects the languages to minify. `.css`, `.js`, and `.mjs` are recognized by
extension, case-insensitively; `.js` minifies as a classic script and `.mjs` as a module. CSS and
JavaScript text can also be minified directly with `minify_css` or `minify_javascript`.

```rust
use tola_minify::{JavaScriptKind, minify_javascript};

// A classic script keeps its top-level names; parameters may be renamed.
let classic = minify_javascript("function add(a, b) { return a + b; }", JavaScriptKind::Classic)?;
assert_eq!(classic, "function add(e,t){return e+t}");

// A module keeps its exports; everything else may be renamed or folded.
let module = minify_javascript("export const answer = 6 * 7;", JavaScriptKind::Module)?;
assert_eq!(module, "export const answer=42;");
```

CSS minification merges equivalent rules and keeps legal comments (`/*! … */`).

JavaScript minification parses, compresses, and mangles. A classic script keeps its top-level
binding names because inline handlers and other scripts on the same page may reference them by
name; a module's top-level bindings may be renamed, unreachable from outside.
