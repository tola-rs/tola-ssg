# tola-config

The configuration protocol shared by Tola crates. Every key has a canonical TOML path, field
presence is recorded separately from values, explicitly written fields are checked against their
lifecycle status, and diagnostics are typed.

```rust
use tola_config::Config;

#[derive(Config)]
#[config(section = "site")]
/// Site metadata configuration.
pub struct SiteConfig {
    /// Site title displayed in browser tab.
    pub title: String,

    /// Language code (BCP 47).
    #[config(default = "en")]
    pub language: String,
}
```

The `Config` derive comes from `tola-config-macros` and is re-exported here. It generates
`FIELDS` field-path accessors and the fallible TOML template methods `try_template`,
`try_template_with_header`, and `try_template_with_header_from`.

- `FieldPath`: one canonical TOML key, validated as non-empty bare keys.
- `ConfigPresence`, `ConfigDiagnostics`, and `ConfigSourceRefusal`: record what a document
  wrote, collect typed diagnostics, and keep a refused source.
- `status::FieldStatus`: checks the lifecycle status of configured fields. Experimental and
  deprecated fields warn; a not-implemented field fails validation.
