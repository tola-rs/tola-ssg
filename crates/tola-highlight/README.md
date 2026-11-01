# tola-highlight

Shared syntax highlighting for Tola. Produces ANSI text for terminals and for
`tui preview`, using the same syntect syntax set that Typst embeds for raw
blocks, plus a Typst-syntax branch for `.typ` sources.

```rust
use tola_highlight::{ThemeChoice, render};

let ansi = render("fn main() {}", "rust", ThemeChoice::Dark);
```
