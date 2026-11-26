use std::path::Path;

use crate::filesystem::root_relative;

pub(crate) fn site_relative(path: &Path, root: &Path) -> Option<String> {
    let rel = root_relative(path, root);
    (!rel.as_os_str().is_empty() && rel != path).then(|| tola_diagnostic::display_path(path, root))
}

/// Locate generated content against the immutable source used by its compilation.
pub(crate) fn source_location(
    world: &tola_typst::TypstWorld,
    span: typst::syntax::Span,
) -> Option<tola_diagnostic::Location> {
    tola_typst::diagnostic::resolve_source_location(world, span)
        .map(|src| diagnostic_location(&src))
}

pub(super) fn relativized_text(text: &str, root: &Path) -> String {
    let mut out = text.to_owned();
    relativize_message(&mut out, root);
    out
}

pub(super) fn source_line(src: tola_typst::SourceLine) -> tola_diagnostic::SourceLine {
    let highlight = src
        .highlight
        .and_then(|range| char_range_to_byte_range(&src.text, range));
    tola_diagnostic::SourceLine {
        start_column: src.start_column,
        start_character: src.start_character,
        ends_line: src.ends_line,
        ..tola_diagnostic::SourceLine::new(src.line_num, src.text, highlight)
    }
}

fn char_range_to_byte_range(text: &str, (start, end): (usize, usize)) -> Option<(usize, usize)> {
    if start >= end {
        return None;
    }
    let byte_offset = |col| {
        text.char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(text.len()))
            .nth(col)
    };
    Some((byte_offset(start)?, byte_offset(end)?))
}

fn diagnostic_location(src: &tola_typst::SourceLocation) -> tola_diagnostic::Location {
    tola_diagnostic::Location {
        file: src
            .path
            .as_deref()
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .map(Path::to_owned),
        path: src.path.clone().unwrap_or_default(),
        line: src.line,
        column: src.column,
        range: src.range.map(|range| tola_diagnostic::SourceRange {
            start: tola_diagnostic::SourcePosition {
                line: range.start.line,
                character: range.start.character,
            },
            end: tola_diagnostic::SourcePosition {
                line: range.end.line,
                character: range.end.character,
            },
        }),
        source_lines: src.source_lines.iter().cloned().map(source_line).collect(),
    }
}

pub(super) fn reported_location(
    loc: &tola_typst::SourceLocation,
    root: &Path,
) -> Option<tola_diagnostic::Location> {
    let path = loc.path.as_ref()?;
    let mut at = diagnostic_location(loc);
    at.path = tola_diagnostic::display_path(Path::new(path), root);
    Some(at)
}

pub(super) fn help_from_hint(hint: tola_typst::Hint, root: &Path) -> tola_diagnostic::Help {
    tola_diagnostic::Help {
        message: hint.message,
        location: reported_location(&hint.location, root),
    }
}

pub(super) fn trace_frame(trace: tola_typst::Trace, root: &Path) -> tola_diagnostic::TraceFrame {
    tola_diagnostic::TraceFrame {
        message: trace.message,
        location: reported_location(&trace.location, root),
    }
}

pub(super) fn relativize_messages(diag: &mut tola_typst::Diagnostic, root: &Path) {
    relativize_message(&mut diag.message, root);
    for hint in &mut diag.hints {
        relativize_message(&mut hint.message, root);
    }
    for trace in &mut diag.traces {
        relativize_message(&mut trace.message, root);
    }
}

pub(super) fn relativize_message(text: &mut String, root: &Path) {
    let root = root.to_string_lossy();
    let root = root.trim_end_matches(['/', '\\']);
    if !root.is_empty() {
        *text = replace_root_prefix(text, root);
    }
}

fn replace_root_prefix(text: &str, root: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find(root) {
        let (before, matched) = rest.split_at(idx);
        let after = &matched[root.len()..];
        let Some(rel) = site_relative_after_root(before, after) else {
            out.push_str(&rest[..idx + root.len()]);
            rest = after;
            continue;
        };
        out.push_str(before);
        for ch in rel.chars() {
            out.push(if ch == '\\' { '/' } else { ch });
        }
        rest = &after[1 + rel.len()..];
    }
    out.push_str(rest);
    out
}

fn site_relative_after_root<'a>(before: &str, after: &'a str) -> Option<&'a str> {
    let continues_component =
        before.chars().next_back().is_some_and(|prev| {
            prev.is_alphanumeric() || matches!(prev, '/' | '\\' | '_' | '-' | '.')
        }) || after
            .chars()
            .next()
            .is_some_and(|next| next.is_alphanumeric() || matches!(next, '.' | '-' | '_'));
    if continues_component {
        return None;
    }
    let rel = relative_path_run(after.strip_prefix(['/', '\\'])?);
    (!rel.is_empty()).then_some(rel)
}

fn relative_path_run(path: &str) -> &str {
    let end = path
        .find(|ch: char| {
            ch.is_whitespace() || matches!(ch, '(' | ')' | '`' | '\'' | '"' | ',' | ';')
        })
        .unwrap_or(path.len());
    &path[..end]
}
