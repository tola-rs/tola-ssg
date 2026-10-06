# tola-config-macros

The proc-macro half of `tola-config`. `#[derive(Config)]` generates field-path accessors
(`FIELDS`) and the fallible TOML template methods (`try_template`, `try_template_with_header`,
`try_template_with_header_from`).

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

    /// Experimental switch.
    #[config(status = experimental)]
    pub dark_mode: bool,

    /// Internal field: not in field paths, templates, or status validation.
    #[config(skip)]
    pub internal: String,
}
```

Attributes: struct-level `section`, `crate`/`tola_config`, `collection`; field-level `skip`,
`hidden`, `name`, `default`, `values`, `collection`, `status`, `sub`. Details are in the macro's
rustdoc.
