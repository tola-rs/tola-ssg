//! Entity roles follow actual public function bindings and explicit string arguments.

use std::ops::Range;

use anyhow::Result;
use tola_build::modules::semantics::{SemanticKind, SemanticParameter, SemanticRole};
use tola_typst::typst::foundations::{Arg, Args, Func, Str, Value};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, Span, Spanned, ast};

use super::inference::{ExpressionResolver, INITIALIZER_DEPTH};
use super::semantic::Semantic;

pub(super) struct EntityBinding {
    pub role: SemanticRole,
    pub function: Func,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntityArgument {
    pub binding: usize,
    pub value: String,
    /// The entire quoted literal, for a definition's selection range.
    pub range: Range<usize>,
    /// The written contents, for completion replacement without duplicating quotes.
    pub content_range: Range<usize>,
}

#[derive(Clone)]
struct Argument {
    name: Option<String>,
    span: Span,
    value: Option<Value>,
    literal: Option<(String, Range<usize>)>,
}

pub(super) struct EntityScan {
    pub literals: Vec<EntityArgument>,
    /// A proven entity callable has an argument whose literal edit cannot be established.
    pub unresolved: bool,
}

/// Reads arguments from syntax nodes, never text occurrences; entity membership stays the caller's.
pub(super) fn scan(
    source: &Source,
    semantics: &mut Semantic<'_>,
    bindings: &[EntityBinding],
) -> Result<EntityScan> {
    let mut found = Vec::new();
    let mut unresolved = false;
    let mut pending = vec![LinkedNode::new(source.root())];
    while let Some(node) = pending.pop() {
        semantics.ensure_active()?;
        unresolved |= match_call(&node, semantics, bindings, &mut found)?;
        pending.extend(node.children());
    }
    Ok(EntityScan {
        literals: in_source_order(found),
        unresolved,
    })
}

/// The arguments in the order their literals appear, each binding's duplicate removed.
fn in_source_order(mut arguments: Vec<EntityArgument>) -> Vec<EntityArgument> {
    arguments.sort_by_key(|argument| (argument.range.start, argument.binding));
    arguments.dedup();
    arguments
}

pub(super) fn argument_at(
    source: &Source,
    cursor: usize,
    semantics: &mut Semantic<'_>,
    bindings: &[EntityBinding],
) -> Result<Vec<EntityArgument>> {
    let root = LinkedNode::new(source.root());
    let mut found = Vec::new();
    for side in [Side::Before, Side::After] {
        let mut current = root.leaf_at(cursor, side);
        while let Some(node) = current {
            semantics.ensure_active()?;
            if node.cast::<ast::FuncCall>().is_some() {
                match_call(&node, semantics, bindings, &mut found)?;
                break;
            }
            current = node.parent().cloned();
        }
    }
    found.retain(|argument| {
        argument.content_range.start <= cursor && cursor <= argument.content_range.end
    });
    Ok(in_source_order(found))
}

fn match_call(
    node: &LinkedNode<'_>,
    semantics: &mut Semantic<'_>,
    bindings: &[EntityBinding],
    found: &mut Vec<EntityArgument>,
) -> Result<bool> {
    let Some(call) = node.cast::<ast::FuncCall>() else {
        return Ok(false);
    };
    let Some(callee) = node.find(call.callee().span()) else {
        return Ok(false);
    };
    let mut unresolved = false;
    for (binding, declared) in bindings.iter().enumerate() {
        let Some(mut applied) = call_arguments(node, call, &callee, declared, semantics)? else {
            if declared.role.role == SemanticKind::Reference
                && is_partial_application(&callee, &declared.function, semantics)?
            {
                unresolved = true;
            }
            continue;
        };
        let Some(written) = written_arguments(node, call, semantics, false)? else {
            unresolved = true;
            continue;
        };
        let inherited = applied.len();
        // Applied values affect mapping, but their own source is handled at their .with call.
        for argument in &mut applied {
            argument.literal = None;
        }
        applied.extend(written);
        if let Some(argument) = selected(&applied, &declared.role.parameter) {
            if let Some((value, range)) = &argument.literal {
                found.push(EntityArgument {
                    binding,
                    value: value.clone(),
                    range: range.clone(),
                    content_range: range.start + 1..range.end - 1,
                });
            } else if !applied[..inherited]
                .iter()
                .any(|prior| std::ptr::eq(prior, argument))
            {
                unresolved = true;
            }
        }
    }
    Ok(unresolved)
}

fn selected<'a>(arguments: &'a [Argument], parameter: &SemanticParameter) -> Option<&'a Argument> {
    match parameter {
        SemanticParameter::Positional { positional } => arguments
            .iter()
            .filter(|argument| argument.name.is_none())
            .nth(*positional),
        SemanticParameter::Named { named } => {
            // Typst's last named argument wins, including an explicit override after .with.
            arguments
                .iter()
                .rev()
                .find(|argument| argument.name.as_deref() == Some(named.as_str()))
        }
    }
}

/// Whether the callee is the named function partly applied with .with.
fn is_partial_application(
    callee: &LinkedNode<'_>,
    function: &Func,
    semantics: &mut Semantic<'_>,
) -> Result<bool> {
    let Some(access) = callee.cast::<ast::FieldAccess>() else {
        return Ok(false);
    };
    if access.field().as_str() != "with" {
        return Ok(false);
    }
    let Some(receiver) = callee.find(access.target().span()) else {
        return Ok(false);
    };
    Ok(applied_arguments(&receiver, function, semantics, INITIALIZER_DEPTH)?.is_some())
}

/// The one function a callee resolves to, when every observed value agrees.
fn resolved_function(node: &LinkedNode<'_>, semantics: &mut Semantic<'_>) -> Result<Option<Func>> {
    if !semantics.callable_is_stable(node)? {
        return Ok(None);
    }
    Ok(match unique_value(node, semantics)? {
        Some(Value::Func(function)) => Some(function),
        _ => None,
    })
}

fn applied_arguments(
    node: &LinkedNode<'_>,
    expected: &Func,
    semantics: &mut Semantic<'_>,
    depth: usize,
) -> Result<Option<Vec<Argument>>> {
    let Some(actual) = resolved_function(node, semantics)? else {
        return Ok(None);
    };
    if &actual == expected {
        return Ok(Some(Vec::new()));
    }
    if depth == 0 {
        return Ok(None);
    }
    if let Some(group) = node.cast::<ast::Parenthesized>()
        && let Some(inner) = node.find(group.expr().span())
    {
        return applied_arguments(&inner, expected, semantics, depth - 1);
    }
    if let Some(call) = node.cast::<ast::FuncCall>()
        && let Some(callee) = node.find(call.callee().span())
        && let Some(access) = callee.cast::<ast::FieldAccess>()
        && access.field().as_str() == "with"
        && let Some(receiver) = callee.find(access.target().span())
        && let Some(receiver_function) = resolved_function(&receiver, semantics)?
        && let Some(mut applied) = applied_arguments(&receiver, expected, semantics, depth - 1)?
        && let Some(written) = written_arguments(node, call, semantics, true)?
    {
        // Func equality compares its actual closure and pre-applied values, not its name/span.
        if preapply(receiver_function, call.span(), &written).as_ref() == Some(&actual) {
            applied.extend(written);
            return Ok(Some(applied));
        }
    }
    if let Some((source, initializer)) = semantics.callable_initializer(node)?
        && let Some(initializer) = LinkedNode::new(source.root()).find(initializer)
        && resolved_function(&initializer, semantics)?.as_ref() == Some(&actual)
    {
        return applied_arguments(&initializer, expected, semantics, depth - 1);
    }
    Ok(None)
}

fn call_arguments(
    node: &LinkedNode<'_>,
    call: ast::FuncCall<'_>,
    callee: &LinkedNode<'_>,
    binding: &EntityBinding,
    semantics: &mut Semantic<'_>,
) -> Result<Option<Vec<Argument>>> {
    let expected = &binding.function;
    if let Some(arguments) = applied_arguments(callee, expected, semantics, INITIALIZER_DEPTH)? {
        return Ok(Some(arguments));
    }
    // Pre-binding a definition parameter does not itself construct the declared entity.
    if binding.role.role == SemanticKind::Definition {
        return Ok(None);
    }
    // A .with call binds parameters explicitly; confirm the resulting function.
    let Some(access) = callee.cast::<ast::FieldAccess>() else {
        return Ok(None);
    };
    if access.field().as_str() != "with" {
        return Ok(None);
    }
    let Some(receiver) = callee.find(access.target().span()) else {
        return Ok(None);
    };
    let Some(receiver_function) = resolved_function(&receiver, semantics)? else {
        return Ok(None);
    };
    let Some(arguments) = applied_arguments(&receiver, expected, semantics, INITIALIZER_DEPTH)?
    else {
        return Ok(None);
    };
    let Some(written) = written_arguments(node, call, semantics, true)? else {
        return Ok(None);
    };
    let Some(applied) = preapply(receiver_function, call.span(), &written) else {
        return Ok(None);
    };
    Ok((resolved_function(node, semantics)? == Some(applied)).then_some(arguments))
}

fn preapply(function: Func, span: Span, arguments: &[Argument]) -> Option<Func> {
    let mut args = Args::new(span, std::iter::empty::<Value>());
    for argument in arguments {
        args.items.push(Arg {
            span: argument.span,
            name: argument.name.as_deref().map(Str::from),
            value: Spanned::new(argument.value.clone()?, argument.span),
        });
    }
    Some(function.with(&mut args))
}

/// One argument a spread supplies: its value is observed, not written at this call site.
fn spread_argument(name: Option<String>, span: Span, value: Value) -> Argument {
    Argument {
        name,
        span,
        value: Some(value),
        literal: None,
    }
}

fn written_arguments(
    node: &LinkedNode<'_>,
    call: ast::FuncCall<'_>,
    semantics: &mut Semantic<'_>,
    resolve_values: bool,
) -> Result<Option<Vec<Argument>>> {
    let mut written = Vec::new();
    for argument in call.args().items() {
        let (name, expression) = match argument {
            ast::Arg::Pos(expression) => (None, expression),
            ast::Arg::Named(named) => (Some(named.name().get().to_string()), named.expr()),
            ast::Arg::Spread(spread) => {
                let Some(expression) = node.find(spread.expr().span()) else {
                    return Ok(None);
                };
                let Some(value) = unique_value(&expression, semantics)? else {
                    return Ok(None);
                };
                match value {
                    Value::Array(array) => {
                        for value in array {
                            written.push(spread_argument(None, spread.span(), value));
                        }
                    }
                    Value::Dict(dictionary) => {
                        for (name, value) in dictionary {
                            written.push(spread_argument(
                                Some(name.to_string()),
                                spread.span(),
                                value,
                            ));
                        }
                    }
                    Value::Args(arguments) => {
                        for argument in arguments.items {
                            written.push(spread_argument(
                                argument.name.map(|name| name.to_string()),
                                spread.span(),
                                argument.value.v,
                            ));
                        }
                    }
                    _ => return Ok(None),
                }
                continue;
            }
        };
        let Some(expression) = node.find(expression.span()) else {
            return Ok(None);
        };
        let literal = expression
            .cast::<ast::Str>()
            .map(|literal| (literal.get().to_string(), expression.range()));
        written.push(Argument {
            name,
            span: expression.span(),
            value: if resolve_values {
                unique_value(&expression, semantics)?
            } else {
                None
            },
            literal,
        });
    }
    Ok(Some(written))
}

fn unique_value(node: &LinkedNode<'_>, semantics: &mut Semantic<'_>) -> Result<Option<Value>> {
    let mut selected = None;
    let values = semantics.known_values(node)?;
    if values.len() >= tola_typst::typst::engine::Sink::MAX_VALUES {
        return Ok(None);
    }
    for (value, _) in values {
        if selected.as_ref().is_some_and(|selected| selected != &value) {
            return Ok(None);
        }
        selected = Some(value);
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::cancellation::BuildCancellation;
    use tola_typst::typst::World;
    use tola_typst::typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

    use crate::query::tests::QuerySession;

    fn matching(program: &str, export: &str, parameter: SemanticParameter) -> Vec<EntityArgument> {
        scanning(program, export, parameter).literals
    }

    fn scanning(program: &str, export: &str, parameter: SemanticParameter) -> EntityScan {
        let mut site = QuerySession::new();
        site.site.write(
            "entities.typ",
            "#let reference(address, text: none) = []\n\
             #let pair(prefix, address) = []\n\
             #let subtree(address: none, body) = []\n\
             #let content = (local: reference)\n",
        );
        site.site.write("content/document.typ", program);
        let cancellation = BuildCancellation::default();
        let revision = site
            .session
            .inspect_source("content/document.typ", Vec::new(), &cancellation)
            .unwrap();
        let compilation = revision.checked().expect("a source world");
        let mut semantics = Semantic::new(compilation, &cancellation);
        let entry = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("/entities.typ").unwrap(),
        )
        .intern();
        let function = semantics.public_export(entry, export).unwrap().unwrap();
        let source = compilation
            .world()
            .source(
                RootedPath::new(
                    VirtualRoot::Project,
                    VirtualPath::new("/content/document.typ").unwrap(),
                )
                .intern(),
            )
            .unwrap();
        scan(
            &source,
            &mut semantics,
            &[EntityBinding {
                role: SemanticRole {
                    export: export.into(),
                    parameter,
                    role: SemanticKind::Reference,
                    query: "entities".into(),
                },
                function,
            }],
        )
        .unwrap()
    }

    fn positional(position: usize) -> SemanticParameter {
        SemanticParameter::Positional {
            positional: position,
        }
    }

    fn values(arguments: &[EntityArgument]) -> Vec<&str> {
        arguments
            .iter()
            .map(|argument| argument.value.as_str())
            .collect()
    }

    #[test]
    fn aliases_follow_the_function_binding() {
        for program in [
            "#import \"/entities.typ\": reference as cite\n#cite(\"QHXS\")",
            "#import \"/entities.typ\" as e\n#e.reference(\"QHXS\")",
            "#import \"/entities.typ\": content\n#content.local(\"QHXS\")",
            "#import \"/entities.typ\": reference\n#let cite = reference\n#cite(\"QHXS\")",
        ] {
            let found = matching(program, "content.local", positional(0));
            assert_eq!(found.len(), 1, "{program}");
            assert_eq!(found[0].value, "QHXS");
            assert_eq!(&program[found[0].range.clone()], "\"QHXS\"");
        }
    }

    #[test]
    fn shadowed_functions_not_entity_references() {
        for program in [
            "#import \"/entities.typ\": reference\n#{ let reference(address) = []; reference(\"QHXS\") }",
            "#import \"/entities.typ\": reference\n#let cite = reference\n#(cite = (address => []))\n#cite(\"QHXS\")",
            "#import \"/entities.typ\": reference\n#let library = (cite: reference)\n#(library.cite = (address => []))\n#library.cite(\"QHXS\")",
            "#import \"/entities.typ\": reference\n#let cite(address) = reference(address)\n#cite(\"QHXS\")",
        ] {
            assert!(
                matching(program, "reference", positional(0)).is_empty(),
                "{program}"
            );
        }
    }

    #[test]
    fn markup_text_not_reassign_callable() {
        let program = "#import \"/entities.typ\": reference\n#let cite = reference\n#cite = (address => [])\n#cite(\"QHXS\")";
        let found = matching(program, "reference", positional(0));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, "QHXS");
    }

    #[test]
    fn alias_calls_keep_the_namespace() {
        let program = "#import \"/entities.typ\" as library\n#let namespace = library\n#namespace.reference(\"FIRST\")\n#namespace.reference(\"QHXS\")";
        let found = matching(program, "reference", positional(0));
        assert_eq!(values(&found), ["FIRST", "QHXS"]);
    }

    #[test]
    fn decoded_strings_keep_written_ranges() {
        let program = "// 雪\n#import \"/entities.typ\": reference\n#reference(\"α\\u{3b2}\\\"\")";
        let found = matching(program, "reference", positional(0));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, "αβ\"");
        assert_eq!(&program[found[0].content_range.clone()], "α\\u{3b2}\\\"");
    }

    #[test]
    fn named_arguments_use_last_value() {
        let program = "#import \"/entities.typ\": subtree\n\
                       #let section = subtree.with(address: \"old\")\n\
                       #section(address: \"new\")[Body]";
        let found = matching(
            program,
            "subtree",
            SemanticParameter::Named {
                named: "address".into(),
            },
        );
        assert_eq!(values(&found), ["old", "new"]);
    }

    #[test]
    fn partial_applications_keep_parameter_positions() {
        let program = "#import \"/entities.typ\": pair\n\
                       #let cite = pair.with(\"prefix\")\n\
                       #cite(\"first\")\n\
                       #pair(..(\"prefix\",), \"second\")";
        let found = matching(program, "pair", positional(1));
        assert_eq!(values(&found), ["first", "second"]);
    }

    #[test]
    fn rename_covers_literal_partial_applications() {
        let supported = scanning(
            "#import \"/entities.typ\": reference\n#let cite = reference.with(\"sku:α\")\n#cite()",
            "reference",
            positional(0),
        );
        assert_eq!(supported.literals.len(), 1);
        assert!(!supported.unresolved);
        for text in [
            "#import \"/entities.typ\": reference\n#let code = \"sku:α\"\n#reference(code)",
            "#import \"/entities.typ\": reference\n#reference(..(\"sku:α\",))",
            "#import \"/entities.typ\": reference\n#reference(..unknown)",
            "#import \"/entities.typ\": reference\n#let code = \"sku:α\"\n#let cite = reference.with(code)\n#cite()",
        ] {
            assert!(
                scanning(text, "reference", positional(0)).unresolved,
                "{text}"
            );
        }
    }

    #[test]
    fn unknown_not_guess_positional_argument() {
        let program = "#import \"/entities.typ\": pair\n#pair(..unknown, \"QHXS\")";
        assert!(matching(program, "pair", positional(1)).is_empty());
    }
}
