# tola-config-macros

`#[derive(Config)]` proc macro for `tola-config`. Generates `FIELDS` key-path accessors and TOML template helpers (`try_template`, `try_template_with_header`, `try_template_with_header_from`).

```rust
#[derive(Config)]
#[config(section = "site")]
/// Site metadata configuration.
pub struct SiteConfig {
    /// Site title displayed in browser tab.
    pub title: String,

    /// Language code (BCP 47).
    #[config(default = "en")]
    pub language: String,

    /// Internal field excluded from templates and validation.
    #[config(skip)]
    pub internal: String,
}
```

Struct attributes: `section`, `crate`/`tola_config`, `collection`. Field attributes: `skip`, `hidden`, `name`, `default`, `values`, `collection`, `status`, `sub`.
