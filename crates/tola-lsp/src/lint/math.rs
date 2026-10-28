//! Math spellings no scope, import, or the checked library defines.

use tola_build::diagnostic::DiagnosticCause;
use tola_typst::typst::Library;
use tola_typst::typst::syntax::LinkedNode;
use tola_typst::typst::syntax::SyntaxKind;
use tola_typst_syntax::names::{OccurrenceKind, SourceNames};

use crate::codes;

use super::Hint;
use super::imports::could_supply_unknown;

/// The math spellings one source leaves undefined, as the checked world's library resolves them.
///
/// Only a `MathIdent` names a variable: a single letter is math text, and a field names a
/// selection. A spelling a lexical declaration, an import, or the math scope binds, or one an
/// import this source cannot enumerate may supply, is not reported.
pub(super) fn warn_undefined_names(names: &SourceNames, library: &Library, hints: &mut Vec<Hint>) {
    let mut stack = vec![LinkedNode::new(names.source().root())];
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::MathIdent && !field_name(&node) {
            warn_undefined_name(names, &node, library, hints);
        }
        for child in node.children() {
            stack.push(child);
        }
    }
}

/// Whether the spelling is the field of a math field access, which names a selection rather than a
/// variable.
fn field_name(node: &LinkedNode<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == SyntaxKind::MathFieldAccess)
        && node
            .prev_sibling()
            .is_some_and(|sibling| sibling.kind() == SyntaxKind::Dot)
}

fn warn_undefined_name(
    names: &SourceNames,
    node: &LinkedNode<'_>,
    library: &Library,
    hints: &mut Vec<Hint>,
) {
    let range = node.range();
    let Some(occurrence) = names.occurrence(range.start) else {
        return;
    };
    if occurrence.range != range
        || !matches!(occurrence.kind, OccurrenceKind::Name)
        || !occurrence.math
    {
        return;
    }
    let name = names.text(&range);
    if name == "std" || names.resolve(name, occurrence.scope, range.start).is_some() {
        return;
    }
    if library.math.scope().get(name).is_some() {
        return;
    }
    if could_supply_unknown(names, occurrence.scope, range.start) {
        return;
    }
    hints.push(Hint {
        code: codes::editor::UNKNOWN_MATH_VARIABLE,
        message: format!("the math name `{name}` is not defined"),
        note: None,
        help: Some(help(name, library.global.scope().get(name).is_some())),
        span: range,
        cause: Some(DiagnosticCause::UnknownVariable {
            name: name.to_owned(),
        }),
    });
}

/// What an author can do about a math spelling no scope defines.
fn help(name: &str, in_library: bool) -> String {
    if matches!(name, "none" | "auto" | "false" | "true") {
        format!("write `#{name}` to use the literal")
    } else if in_library {
        format!("write `#{name}` or `std.{name}` to use the code value")
    } else {
        let separated = name
            .chars()
            .flat_map(|character| [' ', character])
            .skip(1)
            .collect::<String>();
        format!("write each letter separately as `{separated}`, or quote the text as `\"{name}\"`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::tests::{findings, messages};
    use tola_typst::typst::syntax::Source;
    use tola_typst_syntax::names::SourceNames;

    /// The hints one detached source's math spellings justify.
    fn math_hints(text: &str) -> Vec<Hint> {
        let names = SourceNames::new(Source::detached(text.to_owned()));
        let mut hints = Vec::new();
        warn_undefined_names(
            &names,
            &tola_typst::world::library::GLOBAL_LIBRARY,
            &mut hints,
        );
        hints
    }

    /// The math hints one detached source justifies, with each hint's message.
    fn source_messages(text: &str) -> Vec<(String, String)> {
        messages(math_hints(text).into_iter())
    }

    /// The math hints one detached source justifies, as each hint's code, covered source, and the
    /// name its cause states.
    fn pass_findings(text: &str) -> Vec<(String, String, Option<String>)> {
        math_hints(text)
            .into_iter()
            .map(|hint| {
                let name = match hint.cause {
                    Some(DiagnosticCause::UnknownVariable { name }) => Some(name),
                    _ => None,
                };
                (hint.code.to_string(), text[hint.span].to_owned(), name)
            })
            .collect()
    }
    /// A math spelling is hinted only when no scope, import, or library entry defines it: every
    /// spelling a site already establishes stays quiet.
    #[test]
    fn defined_math_names_stay_quiet() {
        for text in [
            "$x$",
            "#let x = 1\n$x$\n",
            "$alpha$",
            "$#x$",
            "$std$",
            "#let alhpa = 1\n$alhpa$",
            "#import \"m.typ\": alhpa\n$alhpa$",
            // A wildcard or dynamic import may supply any name, so the hint would be a guess.
            "#import \"m.typ\": *\n$zzz$",
            "#import modules.at(0)\n$zzz$",
        ] {
            assert!(pass_findings(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn undefined_math_name_is_hinted() {
        assert_eq!(
            pass_findings("$alhpa$"),
            [(
                "editor.unknown_math_variable".to_owned(),
                "alhpa".to_owned(),
                Some("alhpa".to_owned())
            )]
        );
        assert_eq!(
            pass_findings("$none$"),
            [(
                "editor.unknown_math_variable".to_owned(),
                "none".to_owned(),
                Some("none".to_owned())
            )]
        );
        // The library branch names the code spellings; the literal branch names the `#` form.
        let help = &source_messages("$str$")[0].1;
        assert!(
            help.contains("`#str`") && help.contains("`std.str`"),
            "{help}"
        );
        assert!(source_messages("$none$")[0].1.contains("`#none`"));
        // A check that resolved no world cannot tell a name no scope defines from a math builtin.
        assert!(findings("$alhpa$").is_empty());
    }
    #[test]
    fn unevaluated_function_body_is_hinted() {
        assert_eq!(
            pass_findings("#let f() = {\n  $alhpa$\n}\n"),
            [(
                "editor.unknown_math_variable".to_owned(),
                "alhpa".to_owned(),
                Some("alhpa".to_owned())
            )]
        );
    }
}
