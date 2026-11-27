//! Terminal highlighting for Typst API signatures and value declarations.

use std::ops::Range;

use syntect::highlighting::{Highlighter, Style, Theme};
use syntect::parsing::Scope;
use typst_syntax::LinkedNode;

struct Styles {
    keyword: Style,
    name: Style,
    ty: Style,
    string: Style,
    comment: Style,
}

impl Styles {
    fn new(hl: &Highlighter<'_>) -> Self {
        let style = |scope: &str| {
            let scope = Scope::new(scope).expect("valid signature scope");
            hl.style_for_stack(&[scope])
        };
        Self {
            keyword: style("keyword"),
            name: style("entity.name"),
            ty: style("constant"),
            string: style("string"),
            comment: style("comment"),
        }
    }
}

pub(crate) fn collect(
    src: &str,
    th: &Theme,
    hl: &Highlighter<'_>,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let st = Styles::new(hl);
    if let Some(open) = find_signature_header(src, &st, runs) {
        if let Some(close) = find_matching_paren(src, open) {
            paint_params(src, open + 1..close, th, hl, &st, runs);
            paint_return(src, close + 1..src.len(), &st, runs);
        }
        return;
    }
    paint_value_decl(src, th, hl, &st, runs);
}

/// Paint `let <name>(` or `<recv>.<name>(` and return the byte offset of `(`.
fn find_signature_header(
    src: &str,
    st: &Styles,
    runs: &mut Vec<(Range<usize>, Style)>,
) -> Option<usize> {
    let first_line = src.split('\n').next()?;
    let open = first_line.find('(')?;
    let head = &first_line[..open];

    if let Some(rest) = head.strip_prefix("let ") {
        let name_start = 4 + rest.len() - rest.trim_start().len();
        let name_end = head.trim_end().len();
        runs.push((0..3, st.keyword));
        if name_start < name_end {
            runs.push((name_start..name_end, st.name));
        }
        return Some(open);
    }

    if let Some(dot) = head.find('.') {
        let name_start = dot + 1;
        let name_end = head.trim_end().len();
        if dot > 0 && name_start < name_end {
            runs.push((name_start..name_end, st.name));
            return Some(open);
        }
    }

    None
}

fn find_matching_paren(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if b == b'\\' {
                i += 1;
            } else if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn paint_params(
    src: &str,
    range: Range<usize>,
    th: &Theme,
    hl: &Highlighter<'_>,
    st: &Styles,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let mut off = range.start;
    for line in src[range].split_inclusive('\n') {
        let line_len = line.len();
        let trimmed_end = line.trim_end_matches(['\r', '\n', ' ', '\t']);
        let body = trimmed_end.strip_suffix(',').unwrap_or(trimmed_end);
        let leading = body.len() - body.trim_start().len();
        let content = &body[leading..];
        let start = off + leading;

        if !content.is_empty() {
            let mut pos = start;
            let rest = if let Some(after) = content.strip_prefix("..") {
                runs.push((pos..pos + 2, st.keyword));
                pos += 2;
                after
            } else {
                content
            };
            if let Some(colon) = rest.find(':') {
                let after_colon = pos + colon + 1;
                let tail_end = start + content.len();
                paint_type_and_default(src, after_colon..tail_end, th, hl, st, runs);
            }
        }

        off += line_len;
    }
}

fn paint_return(
    src: &str,
    range: Range<usize>,
    st: &Styles,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let tail = src[range.clone()].trim_end_matches(['\r', '\n', ' ', '\t', ';']);
    let leading = tail.len() - tail.trim_start().len();
    let trimmed = &tail[leading..];
    let op_start = range.start + leading;

    let (op_len, after_op) = if let Some(rest) = trimmed.strip_prefix("->") {
        (2, rest)
    } else if let Some(rest) = trimmed.strip_prefix('=') {
        (1, rest)
    } else {
        return;
    };

    runs.push((op_start..op_start + op_len, st.keyword));
    let ty_start = op_start + op_len;
    let ty_end = ty_start + after_op.len();
    paint_type(src, ty_start..ty_end, st, runs);
}

fn paint_value_decl(
    src: &str,
    th: &Theme,
    hl: &Highlighter<'_>,
    st: &Styles,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let Some(colon) = src.find(':') else {
        paint_expr(src, 0..src.len(), th, hl, runs);
        return;
    };
    let name_end = src[..colon].trim_end().len();
    let name_start = src[..name_end].len() - src[..name_end].trim_start().len();
    if name_start < name_end {
        runs.push((name_start..name_end, st.name));
    }

    let rest = &src[colon + 1..];
    if find_top_level_eq(rest).is_some() {
        paint_type_and_default(src, colon + 1..src.len(), th, hl, st, runs);
        return;
    }

    let trimmed = rest.trim_end();
    if trimmed.ends_with(')')
        && let Some(note_rel) = trimmed.rfind(" (")
    {
        let note_start = colon + 1 + note_rel + 1;
        let note_end = colon + 1 + trimmed.len();
        paint_type(src, colon + 1..note_start, st, runs);
        runs.push((note_start..note_end, st.comment));
        return;
    }

    paint_type(src, colon + 1..src.len(), st, runs);
}

fn paint_type_and_default(
    src: &str,
    range: Range<usize>,
    th: &Theme,
    hl: &Highlighter<'_>,
    st: &Styles,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    if let Some(eq_rel) = find_top_level_eq(&src[range.clone()]) {
        let eq = range.start + eq_rel;
        paint_type(src, range.start..eq, st, runs);
        runs.push((eq..eq + 1, st.keyword));
        paint_expr(src, eq + 1..range.end, th, hl, runs);
    } else {
        paint_type(src, range, st, runs);
    }
}

fn find_top_level_eq(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if b == b'\\' {
                i += 1;
            } else if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b'=' if depth == 0
                    && bytes.get(i + 1) != Some(&b'=')
                    && bytes.get(i + 1) != Some(&b'>') =>
                {
                    return Some(i);
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn paint_type(src: &str, range: Range<usize>, st: &Styles, runs: &mut Vec<(Range<usize>, Style)>) {
    let bytes = src.as_bytes();
    let mut i = range.start;
    while i < range.end {
        let b = bytes[i];
        if b == b'"' {
            let start = i;
            i += 1;
            while i < range.end {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                i += 1;
                if bytes[i - 1] == b'"' {
                    break;
                }
            }
            runs.push((start..i.min(range.end), st.string));
        } else if b == b'|' {
            runs.push((i..i + 1, st.keyword));
            i += 1;
        } else if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            i += 1;
            while i < range.end
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
            {
                i += 1;
            }
            runs.push((start..i, st.ty));
        } else {
            i += 1;
        }
    }
}

fn paint_expr(
    src: &str,
    range: Range<usize>,
    th: &Theme,
    hl: &Highlighter<'_>,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let slice = &src[range.clone()];
    let root = typst_syntax::parse_code(slice);
    crate::walk_typst(
        &LinkedNode::new(&root),
        slice,
        th,
        hl,
        &mut Vec::new(),
        range.start,
        runs,
        true,
    );
}

#[cfg(test)]
mod tests {
    use crate::{ThemeChoice, highlight_typst_signature};

    fn render_sig(src: &str) -> String {
        let hl = highlight_typst_signature(src, ThemeChoice::Terminal);
        let mut out = String::new();
        hl.paint(0..src.len(), &mut out);
        out
    }

    #[test]
    fn signature_union_default_no_invalid_highlight() {
        let cases = [
            "let icon-url(\n  id: str,\n  module: str | none = none,\n) = str;",
            "let img(\n  src: str,\n  ..attrs: arguments,\n  format: \"webp\" | \"png\" = \"webp\",\n) = content;",
            "r.atom(\n  name: str,\n) -> bool",
            "site: dictionary (site values)",
            "count: int = 42",
        ];
        for src in cases {
            let ansi = render_sig(src);
            assert!(ansi.contains("\x1b["), "expected ANSI styling for:\n{src}");
            assert!(
                !ansi.contains("\x1b[31") && !ansi.contains(";4m"),
                "unexpected invalid/red-underline styling for:\n{src}\n{ansi:?}"
            );
        }
    }
}
