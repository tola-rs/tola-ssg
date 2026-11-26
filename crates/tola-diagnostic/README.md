# tola-diagnostic

Structured diagnostics shared by Tola's CLI, dev overlay, LSP, and JSON output.

Every code is `<surface>.<contract>` and is declared in [`codes`].

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

- Attach diagnostics to `anyhow::Error` with `DiagnosticError::attach` and extract them with `attached`.
- Enable the optional `json-schema` feature to derive `schemars::JsonSchema`.
