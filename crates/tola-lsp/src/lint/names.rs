//! Spelling suggestions for unbound Typst names.

use std::ops::Range;
use std::path::Path;

use tola_diagnostic::{Diagnostic, DiagnosticCause, Help};
use tola_typst::typst::Library;
use tola_typst_syntax::names::{Occurrence, OccurrenceKind, SourceNames};
use tola_typst_syntax::position::{byte_offset, checked_position};

/// The unbound occurrence a report still describes in this source revision.
pub(crate) fn unbound<'a>(
    names: &'a SourceNames,
    range: &Range<usize>,
    name: &str,
) -> Option<&'a Occurrence> {
    if !tola_typst_syntax::typst_syntax::is_ident(name)
        || names.source().text().get(range.clone()) != Some(name)
    {
        return None;
    }
    let occurrence = names.occurrence(range.start)?;
    (occurrence.range == *range
        && matches!(occurrence.kind, OccurrenceKind::Name)
        && names.resolve(name, occurrence.scope, range.start).is_none())
    .then_some(occurrence)
}

/// One close spelling from the lexical scopes and the standard library visible to the name.
pub(crate) fn replacement<'a>(
    names: &'a SourceNames,
    occurrence: &Occurrence,
    library: &'a Library,
) -> Option<&'a str> {
    let at = occurrence.range.start;
    if super::imports::could_supply_unknown(names, occurrence.scope, at) {
        return None;
    }
    let scopes = names.scopes();
    let local = std::iter::successors(Some(occurrence.scope), |scope| scopes[*scope].parent)
        .flat_map(|scope| &scopes[scope].declarations)
        .map(|&index| &names.declarations()[index])
        .filter(|declared| declared.visible_at(at))
        .map(|declared| declared.name.as_str());
    let builtin = if occurrence.math {
        library.math.scope()
    } else {
        library.global.scope()
    };
    let candidates = local
        .chain(builtin.iter().map(|(name, _)| name.as_str()))
        .chain(std::iter::once("std"))
        .filter(|name| !occurrence.math || name.chars().count() > 1);
    tola_typst::spelling::closest(names.text(&occurrence.range), candidates)
}

/// Put the suggested spelling on the compiler's diagnostic rather than publish another warning.
pub(crate) fn add_spelling_help(
    diagnostics: &mut [Diagnostic],
    path: &Path,
    root: &Path,
    names: &SourceNames,
    library: &Library,
) {
    for diagnostic in diagnostics {
        let Some(DiagnosticCause::UnknownVariable { name }) = &diagnostic.cause else {
            continue;
        };
        let Some(location) = &diagnostic.location else {
            continue;
        };
        if location.editor_file(root).as_deref() != Some(path) {
            continue;
        }
        let Some(range) = location.range else {
            continue;
        };
        let lines = names.source().lines();
        let offset = |at: tola_diagnostic::SourcePosition| {
            byte_offset(lines, checked_position(at.line, at.character)?).ok()
        };
        let (Some(start), Some(end)) = (offset(range.start), offset(range.end)) else {
            continue;
        };
        let Some(occurrence) = unbound(names, &(start..end), name) else {
            continue;
        };
        let Some(replacement) = replacement(names, occurrence, library) else {
            continue;
        };
        let quoted = format!("`{replacement}`");
        if !diagnostic
            .help
            .iter()
            .any(|help| help.message.contains(&quoted))
        {
            diagnostic.help.insert(
                0,
                Help {
                    message: format!("Check whether you meant {quoted}"),
                    location: None,
                },
            );
        }
    }
}
