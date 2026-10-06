//! The value a parameter provably takes, resolved from the calls to its function.

use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{FileId, LinkedNode, Source, VirtualRoot};

use super::super::semantic::{self, Semantic};
use super::declared::{DeclaredOutput, declared_chain_type, declared_output};
use super::origin::{RecordOrigin, chained_origin, field_chain, record_origin};

/// The value one parameter provably takes, resolved from the calls to its function.
#[derive(Clone)]
pub(in crate::query) struct ParameterArgument {
    /// The declared output the argument's chain answers.
    pub(in crate::query) declared: Option<DeclaredOutput>,
    /// The type the argument reads as: what its chain declares, or what the world observed at it.
    pub(in crate::query) ty: Option<String>,
    /// The record the argument's value is, when its chain reads one: a chain over the parameter
    /// continues through this origin.
    pub(in crate::query) origin: Option<RecordOrigin>,
}

/// The named function one parameter declaration belongs to.
struct FunctionParameter {
    /// The file that declares the function.
    file: FileId,
    /// The bytes of the function's own declaration.
    range: Range<usize>,
    /// The name a call writes.
    name: String,
    /// The parameter's position among the function's positional parameters.
    index: usize,
}

/// The value the site's own call sites prove one parameter takes.
///
/// A parameter's value is the argument each call passes, which the world observed only where a
/// call ran. With no observation, the project-local calls the compiler read still prove it, as
/// long as every call passes the same argument chain: a second call with another shape, a named
/// or spread argument, a call that omits the parameter, or an argument no chain declares leaves
/// the value unproved.
pub(in crate::query) fn parameter_argument(
    prepared: &Source,
    names: &tola_typst_syntax::names::SourceNames,
    declaration: &tola_typst_syntax::names::Declaration,
    semantics: &mut Semantic<'_>,
) -> Result<Option<ParameterArgument>> {
    if !matches!(prepared.id().root(), VirtualRoot::Project) {
        return Ok(None);
    }
    let Some(parameter) = tola_typst_syntax::syntax::node_at_range(prepared, &declaration.range)
    else {
        return Ok(None);
    };
    let Some(function) = function_parameter(names, &parameter) else {
        return Ok(None);
    };
    let mut sites = 0;
    let mut proved: Option<ParameterArgument> = None;
    for file in semantics.project_sources() {
        let Some(source) = semantics.world_source(file) else {
            continue;
        };
        if !source.text().contains(&function.name) {
            continue;
        }
        let Some(file_names) = semantics.source_names(file) else {
            continue;
        };
        for call in function_calls(&file_names) {
            let Some(func) = call.cast::<ast::FuncCall>() else {
                continue;
            };
            let Some(callee) = call.find(func.callee().span()) else {
                continue;
            };
            let Some(ident) = callee.cast::<ast::Ident>() else {
                continue;
            };
            if ident.get() != function.name.as_str() {
                continue;
            }
            let Some(span) = semantics.definition(&callee)? else {
                continue;
            };
            if span.id() != Some(function.file) {
                continue;
            }
            let Some(range) = semantics.span_range(span) else {
                continue;
            };
            if range != function.range {
                continue;
            }
            let arguments = func.args().items().collect::<Vec<_>>();
            if arguments
                .iter()
                .any(|argument| !matches!(argument, ast::Arg::Pos(_)))
            {
                return Ok(None);
            }
            let Some(ast::Arg::Pos(argument)) = arguments.get(function.index) else {
                return Ok(None);
            };
            let Some(argument) = call.find(argument.span()) else {
                return Ok(None);
            };
            let Some(value) = argument_value(&argument, &file_names, semantics)? else {
                return Ok(None);
            };
            match &proved {
                Some(previous)
                    if previous.declared != value.declared || previous.ty != value.ty =>
                {
                    return Ok(None);
                }
                Some(_) => {}
                None => proved = Some(value),
            }
            sites += 1;
        }
    }
    // One call site's argument is the record a chain over the parameter continues through; several
    // agreeing calls still declare the parameter's own shape, but no one chain is theirs.
    Ok(proved.map(|mut proved| {
        if sites > 1 {
            proved.origin = None;
        }
        proved
    }))
}

/// The named function one parameter declaration belongs to, with the parameter's position among
/// its positional parameters.
fn function_parameter(
    names: &tola_typst_syntax::names::SourceNames,
    parameter: &LinkedNode<'_>,
) -> Option<FunctionParameter> {
    let mut node = parameter.clone();
    loop {
        if let Some(closure) = node.cast::<ast::Closure>() {
            let function = closure.name()?;
            let mut index = None;
            let mut positional = 0;
            for param in closure.params().children() {
                match param {
                    ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(ident))) => {
                        if node.find(ident.span())?.range() == parameter.range() {
                            index = Some(positional);
                        }
                        positional += 1;
                    }
                    // A named or rest parameter makes a positional argument's target ambiguous.
                    _ => return None,
                }
            }
            let index = index?;
            let ident = node.find(function.span())?;
            // The function is the declaration the closure's own name writes: a call resolving to
            // another binding of that spelling names a different function.
            let declared = names.declared_at(ident.range().start)?;
            if names.declarations()[declared].range != ident.range() {
                return None;
            }
            return Some(FunctionParameter {
                file: names.source().id(),
                range: ident.range(),
                name: function.get().to_string(),
                index,
            });
        }
        node = node.parent()?.clone();
    }
}

/// Every function call one file writes, in source order.
fn function_calls(names: &tola_typst_syntax::names::SourceNames) -> Vec<LinkedNode<'_>> {
    let mut calls = Vec::new();
    let mut pending = vec![LinkedNode::new(names.source().root())];
    while let Some(node) = pending.pop() {
        pending.extend(node.children());
        if node.is::<ast::FuncCall>() {
            calls.push(node);
        }
    }
    calls
}

/// The value one call argument declares, resolved in the file that writes the call.
fn argument_value<'a>(
    argument: &LinkedNode<'a>,
    names: &'a Arc<tola_typst_syntax::names::SourceNames>,
    semantics: &mut Semantic<'_>,
) -> Result<Option<ParameterArgument>> {
    let (root, fields) = match field_chain(argument) {
        Some((root, fields)) => (root, fields),
        None => (argument.clone(), Vec::new()),
    };
    let path = names.text(&argument.range()).to_owned();
    let origin = record_origin(names, root, semantics)?;
    let mut declared = None;
    let mut ty = None;
    let mut value_origin = None;
    if let Some(origin) = &origin {
        declared = declared_output(origin, &fields, &path, semantics)?;
        ty = declared_chain_type(origin, &fields);
        value_origin = chained_origin(origin, &fields);
    }
    if ty.is_none() {
        ty = semantics
            .values(argument)?
            .first()
            .map(|(value, _)| semantic::type_spelling(&value.ty()));
    }
    Ok(
        (declared.is_some() || ty.is_some()).then_some(ParameterArgument {
            declared,
            ty,
            origin: value_origin,
        }),
    )
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;

    /// A parameter answers its own declaration line; a use answers what its declaration does.
    #[test]
    fn parameters_answer_their_declaration() {
        let mut site = QuerySession::new();
        for (marked, expected) in [
            (
                "#let helper(val|ue, scale: 2) = value * scale\n",
                "let value = any;",
            ),
            (
                "#let helper(value, sca|le: 2) = value * scale\n",
                "let scale = int;",
            ),
            ("#let lonely(par|am) = 1\n", "let param = any;"),
        ] {
            let hover = site
                .hover_text(marked)
                .unwrap_or_else(|| panic!("{marked:?} asks about a parameter"));
            assert!(hover.contains(expected), "{marked:?} answered {hover}");
        }

        let called = "#let helper(value, scale: 2) = value * scale\n#helper(7, 3)\n";
        let declaration = site
            .hover_text(&called.replace("value,", "val|ue,"))
            .expect("a hover for the parameter's declaration");
        let use_site = site
            .hover_text(&called.replace("= value *", "= val|ue *"))
            .expect("a hover for the parameter's use");
        assert!(declaration.contains("let value = int;"), "{declaration}");
        assert_eq!(
            use_site, declaration,
            "a use answers what its declaration does"
        );
    }

    /// A rest parameter answers Typst's `arguments` sink, never the type the checked world
    /// observed at the argument that fills it.
    #[test]
    fn rest_parameter_answers_arguments() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let collect(..values) = values\nCall #collect(val|ues: 1)\n")
            .expect("a hover for the named argument");
        assert!(hover.contains("type: arguments"), "{hover}");
    }

    /// A parameter answers the block above it, where the world observed no value for the name.
    #[test]
    fn parameter_answers_the_block_above_it() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let helper(\n  /// The value to scale.\n  val|ue,\n) = value * 2\n")
            .expect("a hover for the documented parameter");
        assert!(hover.contains("The value to scale."), "{hover}");
    }
    /// A function this file declares answers its parameters' observed types and values, whether
    /// the call spells its arguments by position or by name.
    #[test]
    fn declared_function_answers_observed_parameters() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text(
                "#let helper(value, scale: 2) = value * scale\nCall #hel|per(1, scale: 2)\n",
            )
            .expect("a hover for the function");
        assert_eq!(
            hover,
            "```typc\nlet helper(\n  value: int,\n  scale: int = 2,\n) = any;\n```\n\n---\n\n# Positional Parameters\n\n## value\n\n```typc\ntype: int\n```\n\n# Named Parameters\n\n## scale\n\n```typc\ntype: int\n```"
        );

        // The same call, spelling the value it passes by the parameter's own name.
        let named = site
            .hover_text(
                "#let helper(value, scale: 2) = value * scale\nCall #hel|per(value: 1, scale: 2)\n",
            )
            .expect("a hover for the function");
        assert!(
            named.contains("## value\n\n```typc\ntype: int\n```"),
            "{named}"
        );
        assert!(
            named.contains("## scale\n\n```typc\ntype: int\n```"),
            "{named}"
        );
    }
    /// The name of a named argument answers the parameter it fills, with the value the call bound
    /// there.
    #[test]
    fn named_argument_names_answer_the_parameter() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let scale(value, factor: 2) = value * factor\n#scale(1, fact|or: 3)\n")
            .expect("a hover for the argument's name");
        assert!(hover.contains("## factor"), "{hover}");
        assert!(hover.contains("type: int"), "{hover}");
    }
}
