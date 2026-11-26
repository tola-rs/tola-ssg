//! Documentation attached to Typst declarations, without evaluation.

use typst_syntax::{Source, SyntaxKind};

/// Read the contiguous `///` block immediately preceding a declaration.
/// Shared indentation is removed, while the examples keep their relative indentation.
pub fn declaration_docs(src: &Source, start: usize) -> Option<String> {
    let line = src.lines().byte_to_line(start)?;
    let mut docs = Vec::new();
    for prev in (0..line).rev() {
        let start = src.lines().line_to_byte(prev)?;
        let end = src.lines().line_to_byte(prev + 1)?;
        let text = src.text()[start..end].trim_end();
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

/// A documentation block split into summary, parameters, and return type.
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
/// A fence opens on 3+ backticks or tildes (up to 3 leading spaces) and closes on a matching
/// marker at least as long with no trailing content. Backtick info strings containing `` ` ``
/// are inline code and do not open fences.
pub fn documentation_lines(block: &str) -> impl Iterator<Item = DocumentationLine<'_>> {
    let mut fence: Option<(u8, usize)> = None;
    block.lines().map(move |line| {
        let is_fenced = match fence {
            Some((marker, len)) => {
                if fence_marker(line).is_some_and(|(closing, count, rest)| {
                    closing == marker && count >= len && rest.trim().is_empty()
                }) {
                    fence = None;
                }
                true
            }
            None => {
                if let Some((marker, len, info)) = fence_marker(line)
                    && (marker != b'`' || !info.contains('`'))
                {
                    fence = Some((marker, len));
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

impl Documentation {
    /// Parses `- name (type): description` parameter lines and `-> type` return lines outside
    /// fenced examples.
    pub fn parse(block: &str) -> Self {
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
                && let Some(desc) = text.strip_prefix("->")
            {
                push_line(&mut returns, desc.trim_start());
                part = Part::Returns;
                continue;
            }
            match part {
                Part::Summary => push_line(&mut summary, text),
                Part::Parameter(idx) => push_line(&mut parameters[idx].description, text),
                Part::Returns => push_line(&mut returns, text),
            }
        }
        Self {
            summary: summary.trim().to_owned(),
            parameters: parameters
                .into_iter()
                .map(|param| DocumentedParameter {
                    description: param.description.trim().to_owned(),
                    ..param
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
    let len = rest.bytes().take_while(|&byte| byte == marker).count();
    (len >= 3).then_some((marker, len, &rest[len..]))
}

fn parameter_line(text: &str) -> Option<(String, Option<String>, String)> {
    let sep = text.find(['(', ':'])?;
    let name = text[..sep].trim_end();
    if !typst_syntax::is_ident(name) {
        return None;
    }
    let syntax = typst_syntax::parse_code(name);
    let mut children = syntax.children();
    if children.next()?.kind() != SyntaxKind::Ident || children.next().is_some() {
        return None;
    }
    let rest = &text[sep..];
    let (ty, rest) = if rest.starts_with('(') {
        parse_paren_type(rest)?
    } else {
        (None, rest)
    };
    let description = rest.strip_prefix(':')?.trim_start();
    Some((name.to_owned(), ty, description.to_owned()))
}

fn parse_paren_type(rest: &str) -> Option<(Option<String>, &str)> {
    let mut depth = 1;
    let mut quoted = false;
    let mut escaped = false;
    let mut closing = None;
    for (idx, ch) in rest.char_indices().skip(1) {
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
                    closing = Some(idx);
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
    Some((Some(ty.to_owned()), rest[closing + 1..].trim_start()))
}

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
    fn docs_skip_detached_comment() {
        let src = Source::detached("/// Other declaration.\n\n#let value = 7");
        assert_eq!(
            declaration_docs(&src, src.text().find("#let").unwrap()),
            None
        );
    }

    #[test]
    fn docs_parse_params_and_return() {
        let doc = Documentation::parse(
            "Summary of f.\n\n- x (int): old-style x doc.\n  More about x.\n- y: why.\n-> int",
        );
        assert_eq!(doc.summary, "Summary of f.");
        assert_eq!(
            doc.parameters,
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
        assert_eq!(doc.returns.as_deref(), Some("int"));
    }

    #[test]
    fn docs_unfinished_fence() {
        for opening in ["```typ", "~~~~typ"] {
            let block = format!("Summary.\n{opening}\n- x: example\n-> example return");
            let doc = Documentation::parse(&block);
            assert_eq!(doc.summary, block);
            assert!(doc.parameters.is_empty());
            assert!(doc.returns.is_none());
        }
    }

    #[test]
    fn docs_malformed_params() {
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
            let doc = Documentation::parse(line);
            assert_eq!(doc.summary, line);
            assert!(doc.parameters.is_empty(), "{line}: {doc:?}");
        }
    }
}
