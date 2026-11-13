//! `[formatter]` section configuration.

use serde::{Deserialize, Serialize};
use tola_config::Config;
use tola_typst_syntax::style::{Options, ProseWrap};

/// How Tola writes the site's own `.typ` files, used by `tola fmt` and the language service.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "formatter")]
pub struct FormatterConfig {
    /// Spaces per indentation level. If omitted, use 2.
    pub indent: usize,

    /// Column width for wrapping. If omitted, use 120. Values below 20 use 20.
    #[serde(rename = "print-width")]
    #[config(name = "print-width")]
    pub print_width: usize,

    /// Omit this setting or use `none` to keep existing line breaks in prose. Use `fill` to wrap
    /// to the column width, or `sentence` to put each sentence on its own line.
    #[serde(rename = "prose-wrap")]
    #[config(name = "prose-wrap")]
    pub prose_wrap: ProseWrap,
}

impl Default for FormatterConfig {
    fn default() -> Self {
        let options = Options::default();
        Self {
            indent: options.tab_spaces,
            print_width: options.max_width,
            prose_wrap: options.prose_wrap,
        }
    }
}

impl FormatterConfig {
    /// The options passed to the Typst formatter.
    pub fn options(&self) -> Options {
        Options {
            tab_spaces: self.indent,
            max_width: self.print_width,
            prose_wrap: self.prose_wrap,
        }
    }

    /// What `tola help "[formatter]"` adds under its table.
    pub const HELP: &'static str = r#"
To wrap at 88 columns, add this to `tola.toml`.

```toml
[formatter]
print-width = 88
```

To try it in a site created with `tola init`, put this line in `content/index.typ`:

```typst
#let sizes=(small:8,large:16)
```

Run `tola fmt --check` from the site root. It lists files that need formatting and leaves
them untouched. Run `tola fmt` to apply the changes, then open `content/index.typ` to
see the spaces around the assignment and between the fields. Your editor uses the
same settings when it formats through Tola.

With no site configuration, `tola fmt` uses these defaults on the `.typ` files below
the current directory."#;
}
