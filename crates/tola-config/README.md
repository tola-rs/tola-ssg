# tola-config

Shared TOML configuration types for Tola: key paths (`FieldPath`), field presence tracking (`ConfigPresence`), lifecycle status checks (`status::FieldStatus`), and typed diagnostics (`ConfigDiagnostics`).

Re-exports `#[derive(Config)]` from `tola-config-macros`.

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
