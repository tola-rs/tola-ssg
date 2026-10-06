//! The replacement a formatting request applies to one source.

use lsp_types::{FormattingOptions, Range, TextEdit};
use tola_typst_syntax::edit::Edit;
use tola_typst_syntax::format::{self, Options};
use tola_typst_syntax::typst_syntax::Lines;

use crate::position;
use crate::protocol::FormatterOptions;

/// The indentation used when the editor asks for no tab stop of its own.
const FALLBACK_TAB_SPACES: usize = 2;
/// The widest tab stop a source may be indented by.
const WIDEST_TAB_SPACES: u32 = 8;

/// The edits that format `source`, or `None` when the editor keeps it.
///
/// A source that does not parse and one the printer cannot render answer the same way: the
/// editor keeps the text the author already has.
pub(super) fn edits(
    source: &str,
    options: &FormattingOptions,
    formatter: &FormatterOptions,
) -> Option<Vec<TextEdit>> {
    carried(source, format::format(source, &wanted(options, formatter))?)
}

/// The edits that format the part of `source` one range covers, or `None` when the editor keeps it.
pub(super) fn range_edits(
    source: &str,
    bytes: std::ops::Range<usize>,
    options: &FormattingOptions,
    formatter: &FormatterOptions,
) -> Option<Vec<TextEdit>> {
    carried(
        source,
        format::format_range(source, bytes, &wanted(options, formatter))?,
    )
}

/// One formatting edit, in the protocol's own positions.
fn carried(source: &str, edit: Edit) -> Option<Vec<TextEdit>> {
    Some(vec![TextEdit {
        range: source_range(source, &edit.range)?,
        new_text: edit.text,
    }])
}

/// The style the editor's own request and the site's formatter settings ask for.
fn wanted(options: &FormattingOptions, formatter: &FormatterOptions) -> Options {
    Options {
        tab_spaces: tab_spaces(options),
        max_width: formatter.print_width,
        prose_wrap: formatter.prose_wrap,
    }
}

/// The indentation the editor asked for. Typstyle indents with spaces only, so a request for
/// tabs keeps the fallback rather than an indentation the author did not ask for.
fn tab_spaces(options: &FormattingOptions) -> usize {
    if options.insert_spaces && options.tab_size > 0 {
        options.tab_size.min(WIDEST_TAB_SPACES) as usize
    } else {
        FALLBACK_TAB_SPACES
    }
}

/// The range covering the formatted bytes, in the protocol's UTF-16 positions.
fn source_range(source: &str, bytes: &std::ops::Range<usize>) -> Option<Range> {
    position::utf16_range(&Lines::new(source), bytes.clone())
}
