//! `[formatter]` section configuration.

use serde::{Deserialize, Serialize};
use tola_config::Config;
use tola_typst_syntax::style::ProseWrap;

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
        Self {
            indent: 2,
            print_width: 120,
            prose_wrap: ProseWrap::None,
        }
    }
}

impl FormatterConfig {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_help_example() {
        let config = FormatterConfig::default();

        assert_eq!(config.indent, 2);
        assert_eq!(config.print_width, 120);
        assert_eq!(config.prose_wrap, ProseWrap::None);
    }

    #[test]
    fn site_schema_decodes_the_formatter_table() {
        let source = "[formatter]\nindent = 4\nprint-width = 100\nprose-wrap = \"sentence\"\n";
        let schema: crate::config::SiteConfigSchema = toml::from_str(source).unwrap();

        assert_eq!(schema.formatter.indent, 4);
        assert_eq!(schema.formatter.print_width, 100);
        assert_eq!(schema.formatter.prose_wrap, ProseWrap::Sentence);
    }

    #[test]
    fn partial_declaration_keeps_the_other_defaults() {
        let source = "[formatter]\nprint-width = 88\n";
        let schema: crate::config::SiteConfigSchema = toml::from_str(source).unwrap();

        assert_eq!(schema.formatter.indent, 2);
        assert_eq!(schema.formatter.print_width, 88);
        assert_eq!(schema.formatter.prose_wrap, ProseWrap::None);
    }
}
