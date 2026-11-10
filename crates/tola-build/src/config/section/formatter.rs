//! `[formatter]` section configuration.

use serde::{Deserialize, Serialize};
use tola_config::Config;
use tola_typst_syntax::style::{Options, ProseWrap};

/// How Tola writes the site's own `.typ` files, used by `tola fmt` and the language service.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "formatter")]
pub struct FormatterConfig {
    /// The spaces one nesting level indents by.
    pub indent: usize,

    /// The column the formatter breaks a line at. The formatter wraps no narrower than 20
    /// columns, so a smaller value is raised to that floor rather than refused.
    #[serde(rename = "print-width")]
    #[config(name = "print-width")]
    pub print_width: usize,

    /// How markup prose is wrapped: `none` keeps the author's line breaks, `fill` wraps prose to
    /// the line width, and `sentence` places each sentence on its own line.
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
    pub const HELP: &'static str = "\
`tola fmt` writes the site's own `.typ` files in this style, and the language service uses it for
the file an editor works in:

```toml
[formatter]
indent = 2
print-width = 120
prose-wrap = \"none\"
```

A directory without a `tola.toml` formats under these defaults: run the command there and it
formats that directory's `.typ` files with them.";
}
