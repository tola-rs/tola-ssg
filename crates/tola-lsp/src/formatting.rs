//! The replacement a formatting request applies to one source.
//!
//! The site's `[formatter]` configuration decides indentation, width, and prose wrapping. The
//! editor's per-request `tabSize` and `insertSpaces` decide nothing here: indentation is the
//! site's.

use lsp_types::{Range, TextEdit};
use tola_build::config::section::FormatterConfig;
use tola_typst_syntax::edit::Edit;
use tola_typst_syntax::format::{self, Formatting, Options};
use tola_typst_syntax::typst_syntax::Lines;

use crate::position;

/// The style a formatting answer writes in: the site's `[formatter]` configuration.
///
/// A connection that has not resolved a site's configuration yet formats with the same defaults
/// a site that names no `[formatter]` key gets.
pub(super) fn style(site: Option<&FormatterConfig>) -> Options {
    let defaults = FormatterConfig::default();
    let site = site.unwrap_or(&defaults);
    Options {
        tab_spaces: site.indent,
        max_width: site.print_width,
        prose_wrap: site.prose_wrap,
    }
}

/// The edits that format `source`, or `None` when the editor keeps the text it has.
pub(super) fn edits(source: &str, options: &Options) -> Option<Vec<TextEdit>> {
    match format::format(source, options) {
        Formatting::Reformatted(edit) => carried(source, edit),
        Formatting::Stands | Formatting::Unparsable | Formatting::Unrenderable => None,
    }
}

/// The edits that format the part of `source` one range covers, or `None` when the editor keeps it.
pub(super) fn range_edits(
    source: &str,
    bytes: std::ops::Range<usize>,
    options: &Options,
) -> Option<Vec<TextEdit>> {
    carried(source, format::format_range(source, bytes, options)?)
}

/// One formatting edit, in the protocol's own positions.
fn carried(source: &str, edit: Edit) -> Option<Vec<TextEdit>> {
    Some(vec![TextEdit {
        range: source_range(source, &edit.range)?,
        new_text: edit.text,
    }])
}

/// The range covering the formatted bytes, in the protocol's UTF-16 positions.
fn source_range(source: &str, bytes: &std::ops::Range<usize>) -> Option<Range> {
    position::utf16_range(&Lines::new(source), bytes.clone())
}
