//! Documentation attached to Typst declarations, without evaluation.

use typst_syntax::Source;

/// Read the contiguous `///` block immediately preceding a declaration.
/// Shared indentation is removed, while the examples keep their relative indentation.
pub fn declaration_docs(source: &Source, start: usize) -> Option<String> {
    let line = source.lines().byte_to_line(start)?;
    let mut docs = Vec::new();
    for previous in (0..line).rev() {
        let start = source.lines().line_to_byte(previous)?;
        let end = source.lines().line_to_byte(previous + 1)?;
        let text = source.text()[start..end].trim_end();
        let Some(comment) = text.trim_start().strip_prefix("///") else {
            break;
        };
        docs.push(comment);
    }
    docs.reverse();
    let shared = docs
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or_default();
    (!docs.is_empty()).then(|| {
        docs.iter()
            .map(|line| line.get(shared..).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// One parameter a documentation block describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentedParameter {
    /// The parameter's name, as the block spells it.
    pub name: String,
    /// The type the block writes in parentheses, when it writes one.
    pub ty: Option<String>,
    /// What the block says about the parameter.
    pub description: String,
}

/// A documentation block split into what it says about the declaration itself, the parameters it
/// describes, and the value it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Documentation {
    /// The lines that describe the declaration itself.
    pub summary: String,
    /// The parameters the block describes, in the order it lists them.
    pub parameters: Vec<DocumentedParameter>,
    /// What the block says the declaration returns, when it writes a `->` line.
    pub returns: Option<String>,
}

/// One line of a documentation block, and whether a fenced example covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentationLine<'a> {
    /// The line, without its line break.
    pub text: &'a str,
    /// Whether the line lies inside a fenced example, where no line form applies.
    pub is_fenced: bool,
}

/// The lines of `block`, each marked with whether a fenced example covers it.
///
/// A fence opens on a run of three or more backticks or tildes (up to three leading spaces) and
/// closes on a run of the same marker, at least as long, carrying nothing else. An opening
/// backtick fence whose info text contains a backtick is example text, so it closes nothing.
pub fn documentation_lines(block: &str) -> impl Iterator<Item = DocumentationLine<'_>> {
    let mut fence: Option<(u8, usize)> = None;
    block.lines().map(move |line| {
        let is_fenced = match fence {
            Some((marker, length)) => {
                if fence_marker(line).is_some_and(|(closing, count, rest)| {
                    closing == marker && count >= length && rest.trim().is_empty()
                }) {
                    fence = None;
                }
                true
            }
            None => {
                if let Some((marker, length, info)) = fence_marker(line)
                    && (marker != b'`' || !info.contains('`'))
                {
                    fence = Some((marker, length));
                    true
                } else {
                    false
                }
            }
        };
        DocumentationLine {
            text: line,
            is_fenced,
        }
    })
}

/// One run of a documentation block: its prose, or one fenced example.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentationSegment<'a> {
    /// The prose the block writes outside any fenced example.
    Prose(&'a str),
    /// One fenced example, its fence lines included.
    Example(&'a str),
}

/// Split `block` into its prose and its fenced examples, in the order it writes them.
///
/// A fenced example is code: it reads the same in every language, so a translation replaces the
/// prose around the examples and never an example. Whitespace-only runs are dropped.
pub fn documentation_segments(block: &str) -> Vec<DocumentationSegment<'_>> {
    let lines = documentation_lines(block)
        .zip(block.split_inclusive('\n'))
        .map(|(line, raw)| (line.is_fenced, raw.len()))
        .collect::<Vec<_>>();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    let mut fenced = lines.first().is_some_and(|(fenced, _)| *fenced);
    for (line_fenced, length) in lines {
        if line_fenced != fenced {
            push_segment(&mut segments, &block[start..offset], fenced);
            start = offset;
            fenced = line_fenced;
        }
        offset += length;
    }
    push_segment(&mut segments, &block[start..offset], fenced);
    segments
}

fn push_segment<'a>(segments: &mut Vec<DocumentationSegment<'a>>, text: &'a str, fenced: bool) {
    if text.trim().is_empty() {
        return;
    }
    segments.push(if fenced {
        DocumentationSegment::Example(text)
    } else {
        DocumentationSegment::Prose(text)
    });
}

impl Documentation {
    /// Outside fenced examples, `- name (type): description` describes a Typst parameter and
    /// `-> type` the value the declaration returns, in the shape Tinymist and typlite read. Other
    /// lines continue the part they follow.
    pub fn parse(block: &str) -> Self {
        /// The part of the block that the line being read continues.
        enum Part {
            Summary,
            Parameter(usize),
            Returns,
        }

        let mut summary = String::new();
        let mut parameters: Vec<DocumentedParameter> = Vec::new();
        let mut returns = String::new();
        let mut part = Part::Summary;
        for line in documentation_lines(block) {
            let text = line.text.trim_end();
            if !line.is_fenced
                && let Some((name, ty, description)) =
                    text.strip_prefix("- ").and_then(parameter_line)
            {
                parameters.push(DocumentedParameter {
                    name,
                    ty,
                    description,
                });
                part = Part::Parameter(parameters.len() - 1);
                continue;
            }
            if !line.is_fenced
                && let Some(description) = text.strip_prefix("->")
            {
                push_line(&mut returns, description.trim_start());
                part = Part::Returns;
                continue;
            }
            match part {
                Part::Summary => push_line(&mut summary, text),
                Part::Parameter(index) => push_line(&mut parameters[index].description, text),
                Part::Returns => push_line(&mut returns, text),
            }
        }
        Self {
            summary: summary.trim().to_owned(),
            parameters: parameters
                .into_iter()
                .map(|parameter| DocumentedParameter {
                    description: parameter.description.trim().to_owned(),
                    ..parameter
                })
                .collect(),
            returns: (!returns.trim().is_empty()).then(|| returns.trim().to_owned()),
        }
    }
}

fn fence_marker(line: &str) -> Option<(u8, usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let marker = *rest.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = rest.bytes().take_while(|&byte| byte == marker).count();
    (length >= 3).then_some((marker, length, &rest[length..]))
}

/// Read one `name (type): description` line of a documentation block.
fn parameter_line(text: &str) -> Option<(String, Option<String>, String)> {
    let separator = text.find(['(', ':'])?;
    let name = text[..separator].trim_end();
    if !typst_syntax::is_ident(name) {
        return None;
    }
    let syntax = typst_syntax::parse_code(name);
    let mut children = syntax.children();
    if children.next()?.kind() != typst_syntax::SyntaxKind::Ident || children.next().is_some() {
        return None;
    }
    let rest = &text[separator..];
    let (ty, rest) = if rest.starts_with('(') {
        let mut depth = 1;
        let mut quoted = false;
        let mut escaped = false;
        let mut closing = None;
        for (index, ch) in rest.char_indices().skip(1) {
            if quoted {
                match ch {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => quoted = false,
                    _ => {}
                }
                continue;
            }
            match ch {
                '"' => quoted = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        closing = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let closing = closing?;
        let ty = rest[1..closing].trim();
        if ty.is_empty() {
            return None;
        }
        (Some(ty.to_owned()), rest[closing + 1..].trim_start())
    } else {
        (None, rest)
    };
    let description = rest.strip_prefix(':')?.trim_start();
    Some((name.to_owned(), ty, description.to_owned()))
}

/// Append one line to a part, keeping the blank line before it as the paragraph break it is.
fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_split_prose_from_examples() {
        let block = "Read a value.\n\n```typ\n#let value = 7\n```\n\nThen use it.";
        assert_eq!(
            documentation_segments(block),
            vec![
                DocumentationSegment::Prose("Read a value.\n\n"),
                DocumentationSegment::Example("```typ\n#let value = 7\n```\n"),
                DocumentationSegment::Prose("\nThen use it."),
            ]
        );
    }

    #[test]
    fn tilde_fence_delimits_example() {
        assert_eq!(
            documentation_segments("Text.\n\n~~~\ncode\n~~~\n"),
            vec![
                DocumentationSegment::Prose("Text.\n\n"),
                DocumentationSegment::Example("~~~\ncode\n~~~\n"),
            ]
        );
    }

    #[test]
    fn unclosed_fence_covers_remaining_lines() {
        assert_eq!(
            documentation_segments("Text.\n\n```\ncode"),
            vec![
                DocumentationSegment::Prose("Text.\n\n"),
                DocumentationSegment::Example("```\ncode"),
            ]
        );
    }

    #[test]
    fn examples_keep_relative_indentation() {
        let source = Source::detached(
            "/// Read a value.\n///\n/// ```typ\n/// #let value = {\n///   7\n/// }\n/// ```\n#let value = 7",
        );
        assert_eq!(
            declaration_docs(&source, source.text().rfind("#let").unwrap()).unwrap(),
            "Read a value.\n\n```typ\n#let value = {\n  7\n}\n```"
        );
    }

    #[test]
    fn separated_comments_are_not_attached() {
        let source = Source::detached("/// Other declaration.\n\n#let value = 7");
        assert_eq!(
            declaration_docs(&source, source.text().find("#let").unwrap()),
            None
        );
    }

    #[test]
    fn entries_describe_parameters_and_return() {
        let documentation = Documentation::parse(
            "Summary of f.\n\n- x (int): old-style x doc.\n  More about x.\n- y: why.\n-> int",
        );
        assert_eq!(documentation.summary, "Summary of f.");
        assert_eq!(
            documentation.parameters,
            vec![
                DocumentedParameter {
                    name: "x".to_owned(),
                    ty: Some("int".to_owned()),
                    description: "old-style x doc.\n  More about x.".to_owned(),
                },
                DocumentedParameter {
                    name: "y".to_owned(),
                    ty: None,
                    description: "why.".to_owned(),
                },
            ]
        );
        assert_eq!(documentation.returns.as_deref(), Some("int"));
    }

    #[test]
    fn block_without_entries_keeps_its_summary() {
        let documentation =
            Documentation::parse("Prose about the declaration.\n\n- a plain bullet.");
        assert_eq!(
            documentation.summary,
            "Prose about the declaration.\n\n- a plain bullet."
        );
        assert!(documentation.parameters.is_empty());
        assert!(documentation.returns.is_none());
    }

    #[test]
    fn fenced_examples_keep_their_section() {
        for (opening, closing) in [("```typ", "```"), ("~~~~typ", "~~~~~"), ("  ```", " ```")] {
            let example = format!(
                "{opening}\n- x (int): example parameter\n-> example return\n``\n~~~\n{closing}"
            );
            for prefix in ["Summary.\n", "- value: Description.\n", "-> Return.\n"] {
                let block = format!("{prefix}{example}\n- next: Next parameter.");
                let documentation = Documentation::parse(&block);
                let section = match prefix {
                    "Summary.\n" => &documentation.summary,
                    "- value: Description.\n" => &documentation.parameters[0].description,
                    _ => documentation.returns.as_ref().unwrap(),
                };
                assert!(section.ends_with(&example), "{block:?}: {documentation:?}");
                assert_eq!(documentation.parameters.last().unwrap().name, "next");
                assert!(
                    documentation
                        .parameters
                        .iter()
                        .all(|parameter| parameter.name != "x")
                );
            }
        }
    }

    #[test]
    fn unfinished_fences_preserve_examples() {
        for opening in ["```typ", "~~~~typ"] {
            let block = format!("Summary.\n{opening}\n- x: example\n-> example return");
            let documentation = Documentation::parse(&block);
            assert_eq!(documentation.summary, block);
            assert!(documentation.parameters.is_empty());
            assert!(documentation.returns.is_none());
        }
    }

    #[test]
    fn parameter_types_keep_nested_punctuation() {
        for (name, ty) in [
            ("options", "(title: str, draft: bool)"),
            ("callback", "(int) => str"),
            ("颜色", "str | \"(\" | \"\\\"\""),
            ("font-size", "length"),
        ] {
            let documentation =
                Documentation::parse(&format!("- {name} ({ty}): Meaning: details."));
            assert_eq!(
                documentation.parameters,
                vec![DocumentedParameter {
                    name: name.to_owned(),
                    ty: Some(ty.to_owned()),
                    description: "Meaning: details.".to_owned(),
                }]
            );
        }
    }

    #[test]
    fn malformed_entries_remain_prose() {
        for line in [
            "- a plain bullet: prose",
            "- 7value: prose",
            "- let: prose",
            "- x (int: prose",
            "- x int): prose",
            "- x (int)) : prose",
            "- x (): prose",
            "- x (int) extra: prose",
            "- x (\"unfinished): prose",
        ] {
            let documentation = Documentation::parse(line);
            assert_eq!(documentation.summary, line);
            assert!(
                documentation.parameters.is_empty(),
                "{line}: {documentation:?}"
            );
        }
    }
}
