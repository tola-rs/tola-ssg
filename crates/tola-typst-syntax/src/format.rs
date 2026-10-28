//! Formatting one Typst source.

use crate::edit::Edit;
use crate::style::ProseWrap;
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
    /// How markup prose is wrapped.
    pub prose_wrap: ProseWrap,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tab_spaces: 2,
            max_width: 120,
            prose_wrap: ProseWrap::None,
        }
    }
}

/// What formatting one source produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Formatting {
    /// The source already stands as it should.
    Stands,
    /// The edit that reformats the source.
    Reformatted(Edit),
    /// The source has syntax errors; the author's text stands.
    Unparsable {
        /// The byte offset the tree marks as the first error, when it marks a place.
        offset: Option<usize>,
    },
    /// The printer could not render the parsed source.
    Unrenderable,
}

/// What formatting `source` produced.
///
/// A source that does not parse and one the printer cannot render answer
/// [`Formatting::Unparsable`] and [`Formatting::Unrenderable`], and the author's text stands; only
/// the first is the author's to fix.
pub fn format(source: &str, options: &Options) -> Formatting {
    match Typstyle::new(config(options)).format_text(source).render() {
        Ok(formatted) => match unchanged(0..source.len(), formatted, source) {
            Some(edit) => Formatting::Reformatted(edit),
            None => Formatting::Stands,
        },
        Err(typstyle_core::Error::SyntaxError) => Formatting::Unparsable {
            offset: crate::errors::first_error_range(source).map(|range| range.start),
        },
        Err(typstyle_core::Error::RenderError) => Formatting::Unrenderable,
    }
}

/// The edit that formats the part of `source` the byte `range` covers.
///
/// The formatter widens the range to the constructs it covers, so the edit replaces what it
/// actually reprinted. A source that does not parse answers `None`, like [`format()`].
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
        wrap_mode: match options.prose_wrap {
            ProseWrap::None => WrapMode::None,
            ProseWrap::Fill => WrapMode::Fill,
            ProseWrap::Sentence => WrapMode::Sentence,
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
    fn unparsable_source_names_where_it_fails() {
        let source = "= Title\n#let = \n";
        let broken = source.find("#let").expect("the broken line");
        let line_end = source[broken..]
            .find('\n')
            .map_or(source.len(), |at| broken + at);
        let offset = match format(source, &Options::default()) {
            Formatting::Unparsable { offset } => offset.expect("the tree marks a place"),
            other => panic!("expected an unparsable source, got {other:?}"),
        };
        assert!((broken..=line_end).contains(&offset), "offset {offset}");
    }
}
