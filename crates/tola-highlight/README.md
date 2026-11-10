# tola-highlight

Shared syntax highlighting for Tola. Produces ANSI text for terminals and for
`tui preview`, using the same syntect syntax set that Typst embeds for raw
blocks, plus a Typst-syntax branch for `.typ` sources.

```rust
use tola_highlight::{ThemeChoice, render};

let ansi = render("fn main() {}", "rust", ThemeChoice::Dark);
```

The extra syntax files are also exposed as `code-syntaxes` in `@tola/code`.
Their sources and licenses are recorded in [THIRD-PARTY-GRAMMARS.txt](THIRD-PARTY-GRAMMARS.txt).
