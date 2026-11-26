# tola-minify

Minifies CSS and JavaScript (`.css`, `.js` classic scripts, `.mjs` ES modules).

```rust
use std::path::Path;
use tola_minify::{MinifiedLanguages, MinifyLanguage};

let enabled = MinifiedLanguages::new(true, false);
let css = MinifyLanguage::for_source(Path::new("assets/app.css"), enabled).unwrap();
assert_eq!(css.minify(".a { color: red; } .b { color: red; }").unwrap(), ".a,.b{color:red}");

// Already minified files (`*.min.*`) and disabled languages are skipped.
assert!(MinifyLanguage::for_source(Path::new("assets/app.min.css"), enabled).is_none());
assert!(MinifyLanguage::for_source(Path::new("assets/app.js"), enabled).is_none());
```

Direct helpers `minify_css` and `minify_js` are also available. CSS preserves `/*! ... */` comments; classic JS preserves top-level names while ES modules preserve exports.
