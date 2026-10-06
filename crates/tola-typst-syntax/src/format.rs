//! Formatting one Typst source.

use crate::edit::Edit;
use typst_syntax::Source;
use typstyle_core::{Config, Typstyle, WrapMode};

/// The narrowest width a source may be wrapped to.
const MINIMUM_PRINT_WIDTH: usize = 20;

/// How a caller wants a source formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// The indentation one level uses.
    pub tab_spaces: usize,
    /// The column the formatter wraps at.
    pub max_width: usize,
    /// Whether prose is wrapped to fill each line.
    pub prose_wrap: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tab_spaces: 2,
            max_width: 120,
            prose_wrap: false,
        }
    }
}

/// The edit that formats `source`, or `None` when the source already stands as it should.
///
/// A source that does not parse and one the printer cannot render answer the same way: the caller
/// keeps the text the author already has.
pub fn format(source: &str, options: &Options) -> Option<Edit> {
    let formatted = Typstyle::new(config(options))
        .format_text(source)
        .render()
        .ok()?;
    unchanged(0..source.len(), formatted, source)
}

/// The edit that formats the part of `source` the byte `range` covers.
///
/// The formatter widens the range to the constructs it covers, so the edit replaces what it
/// actually reprinted. A source that does not parse answers `None`, as formatting one does.
pub fn format_range(
    source: &str,
    range: std::ops::Range<usize>,
    options: &Options,
) -> Option<Edit> {
    let reprinted = Typstyle::new(config(options))
        .format_source_range(Source::detached(source.to_owned()), range)
        .ok()?;
    unchanged(reprinted.source_range, reprinted.content, source)
}

/// The edit that replaces `range` with `text`, or `None` when the text already stands.
fn unchanged(range: std::ops::Range<usize>, text: String, source: &str) -> Option<Edit> {
    (text != source[range.clone()]).then_some(Edit {
        range,
        text,
        insertion: None,
    })
}

/// The style a caller's options ask for, in the formatter's own terms.
fn config(options: &Options) -> Config {
    Config {
        tab_spaces: options.tab_spaces,
        max_width: options.max_width.max(MINIMUM_PRINT_WIDTH),
        wrap_mode: if options.prose_wrap {
            WrapMode::Fill
        } else {
            WrapMode::None
        },
        ..Config::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_answers_the_construct_it_covers() {
        let source = "= Head\n\n#let  x =  1\n";
        let start = source.find("#let").expect("the binding");
        let edit = format_range(source, start..source.len(), &Options::default()).expect("an edit");
        assert_eq!(edit.text, "#let x = 1");
        assert_eq!(edit.range, start..source.len() - 1);
    }

    #[test]
    fn unparsable_range_answers_nothing() {
        assert!(format_range("#let = ", 0..7, &Options::default()).is_none());
    }

    #[test]
    fn changed_source_answers_its_edit() {
        let edit = format("#let  x =  1\n", &Options::default()).expect("an edit");
        assert_eq!(edit.text, "#let x = 1\n");
        assert_eq!(edit.range, 0.."#let  x =  1\n".len());
        assert_eq!(edit.insertion, None);
    }

    #[test]
    fn already_formatted_source_answers_none() {
        assert!(format("#let x = 1\n", &Options::default()).is_none());
    }

    #[test]
    fn narrow_width_leaves_formatted_text_alone() {
        let narrow = Options {
            max_width: 1,
            ..Options::default()
        };
        assert!(format("#let x = 1\n", &narrow).is_none());
        assert!(format("#let  x =  1\n", &narrow).is_some());
    }
}
