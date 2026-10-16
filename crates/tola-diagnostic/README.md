# tola-diagnostic

The record every Tola producer emits: a stable code, a severity, a message written for the site
author, and context the author can act on — location, notes, help, related information, and call
trace. It stays structured until a caller renders it, so the terminal, the development overlay,
the editor, and JSON output all read the same value.

A code is written `<surface>.<contract>`, where the surface names the subsystem that produces it.
Every code a Tola library publishes is declared once in [`codes`], and a malformed shape fails to
compile.

## Build one

```rust
use tola_diagnostic::{Diagnostic, Severity, codes};

let diagnostic = Diagnostic::at_path(
    codes::config::INVALID,
    Severity::Error,
    "tola.toml",
    "`build.output` must name a directory inside the site",
)
.with_help("Point `build.output` at a directory below the site root");

assert_eq!(diagnostic.code, "config.invalid");
assert_eq!(
    serde_json::to_value(&diagnostic).unwrap()["code"],
    "config.invalid"
);
```

A producer classifies what a correction reads through `Diagnostic::with_cause`, and a failure that
carries diagnostics travels through an ordinary `anyhow` chain: `DiagnosticError::attach` keeps
them, `attached` finds them again. `fallback` writes the record for an error that arrived
unclassified.

## Rendering

Rendering belongs to the caller. A diagnostic keeps the source lines its producer retained and the
message it stored, and `Diagnostic::display_message` gives the sentence the site shows in place of
Typst's original panic text.

The optional `json-schema` feature derives `schemars::JsonSchema` for every record, matching its
Serde serialization.
