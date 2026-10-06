//! Resolving one bundled export's declaration and documentation block from package source.

use anyhow::{Context, Result, bail};
use tola_typst_syntax::docs::Documentation;
use typst::foundations::{Func, Scope, Value};
use typst::syntax::ast::AstNode;
use typst::syntax::{LinkedNode, Source, SyntaxKind, ast};

use super::RelatedTarget;
use super::declaration::{ExportDocumentation, Parameter, ParameterKind, Signature};
use super::related;

/// A documentation block and the related targets Tola reads out of it, so a re-export inherits
/// or replaces both together.
#[derive(Debug, Clone)]
pub(super) struct DeclaredDocumentation {
    pub(super) documentation: Documentation,
    pub(super) related: Vec<RelatedTarget>,
}
type FileSource = (&'static str, Source);

pub(super) enum Declaration<'a> {
    Native(Func),
    Source(&'a FileSource, LinkedNode<'a>),
    Export(ExportDocumentation),
    Site,
    Constant(Value),
}

pub(super) struct Declarations {
    pub(super) sources: Vec<FileSource>,
    pub(super) natives: Scope,
}

impl Declarations {
    pub(super) fn file(&self, path: &str) -> Result<&FileSource> {
        self.sources
            .iter()
            .find(|(name, _)| *name == path)
            .with_context(|| format!("builtin source `{path}` is absent"))
    }

    pub(super) fn binding<'a>(
        &'a self,
        file: &'a FileSource,
        scope: LinkedNode<'a>,
        name: &str,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>)> {
        let mut before = (!matches!(scope.kind(), SyntaxKind::Code | SyntaxKind::Markup))
            .then(|| scope.range().start);
        let mut scope = Some(scope);
        while let Some(parent) = scope {
            if matches!(parent.kind(), SyntaxKind::Code | SyntaxKind::Markup) {
                for statement in parent.children().rev() {
                    // An initializer sees preceding bindings, including an outer binding with
                    // the same name, rather than itself or declarations written later.
                    if before.is_some_and(|before| statement.range().end > before) {
                        continue;
                    }
                    if let Some(binding) = statement.cast::<ast::LetBinding>() {
                        if binding
                            .kind()
                            .bindings()
                            .iter()
                            .any(|bound| bound.as_str() == name)
                        {
                            let documentation =
                                declared_documentation(&file.1, statement.range().start);
                            let expression = binding
                                .init()
                                .context("bundled binding has no initializer")?;
                            let node = file
                                .1
                                .find(expression.span())
                                .context("bundled initializer has no source")?;
                            let (declaration, inherited) = self.expression(file, node)?;
                            return Ok((declaration, documentation.or(inherited)));
                        }
                    } else if let Some(import) = statement.cast::<ast::ModuleImport>() {
                        let Some(ast::Imports::Items(items)) = import.imports() else {
                            continue;
                        };
                        for item in items.iter() {
                            if item.bound_name().as_str() != name {
                                continue;
                            }
                            let documentation =
                                declared_documentation(&file.1, statement.range().start);
                            let ast::Expr::Str(path) = import.source() else {
                                bail!("bundled import is not a literal path")
                            };
                            let path = path.get();
                            let original = item.original_name();
                            if path.as_str() == "@tola/host:0.0.0" {
                                let declaration = if original.as_str() == "site" {
                                    Declaration::Site
                                } else {
                                    let value = self
                                        .natives
                                        .get(original.as_str())
                                        .with_context(|| {
                                            format!(
                                                "unknown builtin native `{}`",
                                                original.as_str()
                                            )
                                        })?
                                        .read();
                                    match value {
                                        Value::Func(func) => Declaration::Native(func.clone()),
                                        value => Declaration::Constant(value.clone()),
                                    }
                                };
                                return Ok((declaration, documentation));
                            }
                            if path.starts_with("@tola/") {
                                let spec = path.parse().map_err(|error| {
                                    anyhow::anyhow!("invalid bundled package import: {error}")
                                })?;
                                let package = crate::builtin_package(&spec)
                                    .context("unknown bundled package import")?;
                                let mut exports = package
                                    .export_documentation(&[original.as_str().to_owned()])?;
                                let export =
                                    exports.pop().context("bundled import has no declaration")?;
                                let inherited = DeclaredDocumentation {
                                    documentation: export.documentation.clone(),
                                    related: export.related.clone(),
                                };
                                return Ok((
                                    Declaration::Export(export),
                                    documentation.or(Some(inherited)),
                                ));
                            }
                            let base = std::path::Path::new(file.0)
                                .parent()
                                .unwrap_or_else(|| std::path::Path::new(""));
                            let imported = self.file(
                                base.join(path.as_str())
                                    .to_str()
                                    .context("builtin import path is not UTF-8")?,
                            )?;
                            let (declaration, inherited) = self.binding(
                                imported,
                                LinkedNode::new(imported.1.root()),
                                original.as_str(),
                            )?;
                            return Ok((declaration, documentation.or(inherited)));
                        }
                    }
                }
            }
            before = Some(parent.range().start);
            scope = parent.parent().cloned();
        }
        bail!("bundled declaration `{name}` has no source binding")
    }

    fn expression<'a>(
        &'a self,
        file: &'a FileSource,
        node: LinkedNode<'a>,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>)> {
        let Some(expression) = node.cast::<ast::Expr>() else {
            bail!("bundled declaration is not an expression")
        };
        match expression {
            ast::Expr::Ident(ident) => self.binding(file, node.clone(), ident.as_str()),
            ast::Expr::FieldAccess(access) => {
                let target = file
                    .1
                    .find(access.target().span())
                    .context("field target has no source")?;
                let (target, _) = self.expression(file, target)?;
                self.member(target, access.field().as_str())
            }
            ast::Expr::CodeBlock(block) => {
                let final_expression = block
                    .body()
                    .exprs()
                    .next_back()
                    .context("bundled block has no final expression")?;
                self.expression(
                    file,
                    file.1
                        .find(final_expression.span())
                        .context("block result has no source")?,
                )
            }
            ast::Expr::Parenthesized(group) => self.expression(
                file,
                file.1
                    .find(group.expr().span())
                    .context("group has no source")?,
            ),
            _ => Ok((Declaration::Source(file, node), None)),
        }
    }

    fn member<'a>(
        &'a self,
        declaration: Declaration<'a>,
        name: &str,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>)> {
        let Declaration::Source(file, node) = declaration else {
            bail!("bundled member `{name}` has no source dictionary")
        };
        let expression = node
            .cast::<ast::Expr>()
            .context("dictionary is not an expression")?;
        match expression {
            ast::Expr::Dict(dictionary) => {
                for item in dictionary.items().rev() {
                    match item {
                        ast::DictItem::Named(named) if named.name().as_str() == name => {
                            return self.expression(
                                file,
                                file.1
                                    .find(named.expr().span())
                                    .context("dictionary member has no source")?,
                            );
                        }
                        ast::DictItem::Spread(spread) => {
                            let node = file
                                .1
                                .find(spread.expr().span())
                                .context("spread has no source")?;
                            let (spread, _) = self.expression(file, node)?;
                            if let Ok(member) = self.member(spread, name) {
                                return Ok(member);
                            }
                        }
                        _ => {}
                    }
                }
                bail!("bundled dictionary has no member `{name}`")
            }
            ast::Expr::FuncCall(call) => {
                let (callee, _) = self.expression(
                    file,
                    file.1
                        .find(call.callee().span())
                        .context("callee has no source")?,
                )?;
                let Declaration::Source(file, node) = callee else {
                    bail!("bundled dictionary factory is not a source closure")
                };
                let closure = node
                    .cast::<ast::Closure>()
                    .context("bundled dictionary factory is not a closure")?;
                let (returned, _) = self.expression(
                    file,
                    file.1
                        .find(closure.body().span())
                        .context("factory result has no source")?,
                )?;
                self.member(returned, name)
            }
            _ => bail!("bundled member `{name}` is not declared by a dictionary"),
        }
    }
}

/// The documentation block immediately preceding a declaration: the parts every surface reads,
/// and the related targets `related` reads out of it.
pub(super) fn declared_documentation(
    source: &Source,
    start: usize,
) -> Option<DeclaredDocumentation> {
    let block = tola_typst_syntax::docs::declaration_docs(source, start)?;
    let related = related::read(&block);
    Some(DeclaredDocumentation {
        documentation: Documentation::parse(&related.prose),
        related: related.targets,
    })
}

/// The parameters and return one bundled closure declares, with the types and documentation its
/// doc block annotates.
pub(super) fn source_function(
    name: &str,
    file: &FileSource,
    closure: &ast::Closure<'_>,
    documentation: &Documentation,
) -> Result<Signature> {
    let mut parameters = closure
        .params()
        .children()
        .map(|parameter| source_parameter(file, parameter))
        .collect::<Result<Vec<_>>>()?;
    for documented in &documentation.parameters {
        let Some(parameter) = parameters
            .iter_mut()
            .find(|parameter| parameter.name == documented.name)
        else {
            bail!(
                "`{name}` documents parameter `{}`, which the bundled declaration does not have",
                documented.name
            );
        };
        // A rest parameter's value is Typst's `arguments`, which no annotation replaces.
        if parameter.kind != ParameterKind::Rest {
            parameter.ty.clone_from(&documented.ty);
        }
        parameter.docs =
            (!documented.description.is_empty()).then(|| documented.description.clone());
    }
    Ok(Signature {
        name: name.to_owned(),
        parameters,
        returns: documentation.returns.clone(),
    })
}

/// One parameter a bundled closure declares, as its own signature spells it.
fn source_parameter(file: &FileSource, parameter: ast::Param<'_>) -> Result<Parameter> {
    let (name, kind, default) = match parameter {
        ast::Param::Pos(pattern) => (
            pattern_name(file, &pattern)?,
            ParameterKind::Positional,
            None,
        ),
        ast::Param::Named(named) => (
            named.name().as_str().to_owned(),
            ParameterKind::Named,
            Some(named_default(file, &named)?),
        ),
        ast::Param::Spread(spread) => (
            spread
                .sink_ident()
                .map_or_else(String::new, |sink| sink.as_str().to_owned()),
            ParameterKind::Rest,
            None,
        ),
    };
    Ok(Parameter {
        name,
        ty: None,
        default,
        docs: None,
        kind,
    })
}

/// The name a positional parameter binds, as its own source spells it.
fn pattern_name(file: &FileSource, pattern: &ast::Pattern<'_>) -> Result<String> {
    Ok(match pattern {
        ast::Pattern::Normal(ast::Expr::Ident(ident)) => ident.as_str().to_owned(),
        _ => source_text(file, pattern.span())?,
    })
}

/// The default one named parameter declares, as its own source spells it.
fn named_default(file: &FileSource, named: &ast::Named<'_>) -> Result<String> {
    let expression = named
        .to_untyped()
        .children()
        .next_back()
        .and_then(|child| child.cast::<ast::Expr>());
    let Some(expression) = expression else {
        // `name:` is shorthand for a parameter that defaults to `none`.
        return Ok("none".to_owned());
    };
    source_text(file, expression.span())
}

/// The bundled source text of one span, with line breaks folded into single spaces.
fn source_text(file: &FileSource, span: typst::syntax::Span) -> Result<String> {
    let node = file.1.find(span).context("bundled source is absent")?;
    Ok(file.1.text()[node.range()]
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" "))
}
#[cfg(test)]
mod tests {
    use super::*;

    use super::super::declaration::tests::parameter;
    #[test]
    fn documented_types_reach_the_declaration() {
        let declarations = Declarations {
            sources: vec![(
                "lib.typ",
                Source::detached(
                    "/// - value (int | none): what it takes.\n\
                     /// - label (string): how it is named.\n\
                     /// -> array\n\
                     #let sample(value: 1, label: \"x\") = (value,)",
                ),
            )],
            natives: crate::library::native_scope(),
        };
        let file = declarations.file("lib.typ").unwrap();
        let (declaration, docs) = declarations
            .binding(file, LinkedNode::new(file.1.root()), "sample")
            .unwrap();
        let Declaration::Source(file, node) = declaration else {
            panic!("sample should be a source closure")
        };
        let closure = node.cast::<ast::Closure>().unwrap();
        let documentation = docs.unwrap().documentation;
        let signature = source_function("sample", file, &closure, &documentation).unwrap();
        assert_eq!(
            parameter(&signature, "value").ty.as_deref(),
            Some("int | none")
        );
        assert_eq!(
            parameter(&signature, "value").docs.as_deref(),
            Some("what it takes.")
        );
        assert_eq!(parameter(&signature, "value").default.as_deref(), Some("1"));
        assert_eq!(parameter(&signature, "label").ty.as_deref(), Some("string"));
        assert_eq!(signature.returns.as_deref(), Some("array"));
        assert!(documentation.summary.is_empty());
    }

    #[test]
    fn documentation_of_undeclared_parameters_is_rejected() {
        let declarations = Declarations {
            sources: vec![(
                "lib.typ",
                Source::detached("/// - missing (int): nowhere.\n#let sample(value) = value"),
            )],
            natives: crate::library::native_scope(),
        };
        let file = declarations.file("lib.typ").unwrap();
        let (declaration, docs) = declarations
            .binding(file, LinkedNode::new(file.1.root()), "sample")
            .unwrap();
        let Declaration::Source(file, node) = declaration else {
            panic!("sample should be a source closure")
        };
        let closure = node.cast::<ast::Closure>().unwrap();
        let error =
            source_function("sample", file, &closure, &docs.unwrap().documentation).unwrap_err();
        assert!(error.to_string().contains("`missing`"), "{error}");
    }

    #[test]
    fn import_aliases_keep_attached_documentation() {
        let declarations = Declarations {
            sources: vec![(
                "lib.typ",
                Source::detached(
                    "/// Address name.\n#import \"@tola/host:0.0.0\": slugify as name",
                ),
            )],
            natives: crate::library::native_scope(),
        };
        let file = declarations.file("lib.typ").unwrap();
        let (declaration, docs) = declarations
            .binding(file, LinkedNode::new(file.1.root()), "name")
            .unwrap();
        assert_eq!(docs.unwrap().documentation.summary, "Address name.");
        let Declaration::Native(func) = declaration else {
            panic!("alias should retain the native function")
        };
        assert!(!Signature::of("name", &func).parameters.is_empty());
    }

    #[test]
    fn aliases_keep_lexical_signatures() {
        let declarations = Declarations {
            sources: vec![(
                "lib.typ",
                Source::detached(
                    "#let original(first, fallback: none) = first\n\
                     #let alias = original\n\
                     #let original(second) = second\n\
                     #let original = original",
                ),
            )],
            natives: crate::library::native_scope(),
        };
        let file = declarations.file("lib.typ").unwrap();
        for (name, expected) in [("alias", "first, fallback: none"), ("original", "second")] {
            let (declaration, _) = declarations
                .binding(file, LinkedNode::new(file.1.root()), name)
                .unwrap();
            let Declaration::Source(_, node) = declaration else {
                panic!("{name} should refer to its preceding source closure")
            };
            let closure = node.cast::<ast::Closure>().unwrap();
            let parameters = file.1.find(closure.params().span()).unwrap();
            assert_eq!(&file.1.text()[parameters.range()], format!("({expected})"));
        }
    }
}
