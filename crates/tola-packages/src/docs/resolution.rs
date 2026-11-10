//! Resolving one bundled export's declaration and documentation block from package source.

use tola_typst_syntax::docs::Documentation;
use typst::foundations::{Func, Scope, Value};
use typst::syntax::ast::AstNode;
use typst::syntax::{LinkedNode, Source, SyntaxKind, ast};

use super::declaration::{ExportDocumentation, Signature};
use super::related;
use super::{ExportDocumentationError, RelatedTarget};
use ExportDocumentationError::{
    InvalidDeclaration, InvalidImport, MemberOutsideDictionary, MissingBinding, MissingDictionary,
    MissingMember, MissingSource, UnknownNative,
};

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
    pub(super) fn file(&self, path: &str) -> Result<&FileSource, ExportDocumentationError> {
        self.sources
            .iter()
            .find(|(name, _)| *name == path)
            .ok_or_else(|| MissingSource(path.to_owned()))
    }

    pub(super) fn binding<'a>(
        &'a self,
        file: &'a FileSource,
        scope: LinkedNode<'a>,
        name: &str,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>), ExportDocumentationError> {
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
                        if !binding
                            .kind()
                            .bindings()
                            .iter()
                            .any(|bound| bound.as_str() == name)
                        {
                            continue;
                        }
                        let documentation =
                            declared_documentation(&file.1, statement.range().start);
                        let expression = binding
                            .init()
                            .ok_or(InvalidDeclaration("bundled binding has no initializer"))?;
                        let node = file
                            .1
                            .find(expression.span())
                            .ok_or(InvalidDeclaration("bundled initializer has no source"))?;
                        let (declaration, inherited) = self.expression(file, node)?;
                        return Ok((declaration, documentation.or(inherited)));
                    }
                    let Some(import) = statement.cast::<ast::ModuleImport>() else {
                        continue;
                    };
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
                            return Err(InvalidDeclaration("bundled import is not a literal path"));
                        };
                        let path = path.get();
                        let original = item.original_name();
                        if path.as_str() == "@tola/host:0.0.0" {
                            if original.as_str() == "site" {
                                return Ok((Declaration::Site, documentation));
                            }
                            let value = self
                                .natives
                                .get(original.as_str())
                                .ok_or_else(|| UnknownNative(original.as_str().to_owned()))?
                                .read();
                            let declaration = match value {
                                Value::Func(func) => Declaration::Native(func.clone()),
                                value => Declaration::Constant(value.clone()),
                            };
                            return Ok((declaration, documentation));
                        }
                        if path.starts_with("@tola/") {
                            let spec = path
                                .parse::<tola_typst::PackageSpec>()
                                .map_err(|error| InvalidImport(error.to_string()))?;
                            let package = crate::builtin_package(&spec)
                                .ok_or(InvalidDeclaration("unknown bundled package import"))?;
                            let mut exports =
                                package.export_documentation(&[original.as_str().to_owned()])?;
                            let export = exports
                                .pop()
                                .ok_or(InvalidDeclaration("bundled import has no declaration"))?;
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
                                .ok_or(InvalidDeclaration("builtin import path is not UTF-8"))?,
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
            before = Some(parent.range().start);
            scope = parent.parent().cloned();
        }
        Err(MissingBinding(name.to_owned()))
    }

    fn expression<'a>(
        &'a self,
        file: &'a FileSource,
        node: LinkedNode<'a>,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>), ExportDocumentationError> {
        let Some(expression) = node.cast::<ast::Expr>() else {
            return Err(InvalidDeclaration(
                "bundled declaration is not an expression",
            ));
        };
        match expression {
            ast::Expr::Ident(ident) => self.binding(file, node.clone(), ident.as_str()),
            ast::Expr::FieldAccess(access) => {
                let target = file
                    .1
                    .find(access.target().span())
                    .ok_or(InvalidDeclaration("field target has no source"))?;
                let (target, _) = self.expression(file, target)?;
                self.member(target, access.field().as_str())
            }
            ast::Expr::CodeBlock(block) => {
                let final_expression = block
                    .body()
                    .exprs()
                    .next_back()
                    .ok_or(InvalidDeclaration("bundled block has no final expression"))?;
                self.expression(
                    file,
                    file.1
                        .find(final_expression.span())
                        .ok_or(InvalidDeclaration("block result has no source"))?,
                )
            }
            ast::Expr::Parenthesized(group) => self.expression(
                file,
                file.1
                    .find(group.expr().span())
                    .ok_or(InvalidDeclaration("group has no source"))?,
            ),
            _ => Ok((Declaration::Source(file, node), None)),
        }
    }

    fn member<'a>(
        &'a self,
        declaration: Declaration<'a>,
        name: &str,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>), ExportDocumentationError> {
        let Declaration::Source(file, node) = declaration else {
            return Err(MissingDictionary(name.to_owned()));
        };
        let expression = node
            .cast::<ast::Expr>()
            .ok_or(InvalidDeclaration("dictionary is not an expression"))?;
        match expression {
            ast::Expr::Dict(dictionary) => {
                for item in dictionary.items().rev() {
                    match item {
                        ast::DictItem::Named(named) if named.name().as_str() == name => {
                            return self.expression(
                                file,
                                file.1
                                    .find(named.expr().span())
                                    .ok_or(InvalidDeclaration("dictionary member has no source"))?,
                            );
                        }
                        ast::DictItem::Spread(spread) => {
                            let node = file
                                .1
                                .find(spread.expr().span())
                                .ok_or(InvalidDeclaration("spread has no source"))?;
                            let (spread, _) = self.expression(file, node)?;
                            if let Ok(member) = self.member(spread, name) {
                                return Ok(member);
                            }
                        }
                        _ => {}
                    }
                }
                Err(MissingMember(name.to_owned()))
            }
            ast::Expr::FuncCall(call) => {
                let (callee, _) = self.expression(
                    file,
                    file.1
                        .find(call.callee().span())
                        .ok_or(InvalidDeclaration("callee has no source"))?,
                )?;
                let Declaration::Source(file, node) = callee else {
                    return Err(InvalidDeclaration(
                        "bundled dictionary factory is not a source closure",
                    ));
                };
                let closure = node.cast::<ast::Closure>().ok_or(InvalidDeclaration(
                    "bundled dictionary factory is not a closure",
                ))?;
                let (returned, _) = self.expression(
                    file,
                    file.1
                        .find(closure.body().span())
                        .ok_or(InvalidDeclaration("factory result has no source"))?,
                )?;
                self.member(returned, name)
            }
            _ => Err(MemberOutsideDictionary(name.to_owned())),
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
) -> Result<Signature, ExportDocumentationError> {
    let signature = Signature::from_source(name, &file.1, closure, documentation)
        .ok_or(InvalidDeclaration("bundled source is absent"))?;
    for documented in &documentation.parameters {
        if !signature
            .parameters
            .iter()
            .any(|parameter| parameter.name == documented.name)
        {
            return Err(ExportDocumentationError::UnknownParameter {
                name: name.to_owned(),
                parameter: documented.name.clone(),
            });
        }
    }
    Ok(signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::declaration::Parameter;

    fn declarations(source: &str) -> Declarations {
        Declarations {
            sources: vec![("lib.typ", Source::detached(source))],
            natives: crate::library::native_scope(),
        }
    }

    fn parameter<'a>(signature: &'a Signature, name: &str) -> &'a Parameter {
        signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == name)
            .unwrap_or_else(|| panic!("missing parameter `{name}`"))
    }

    #[test]
    fn documented_types_reach_the_declaration() {
        let declarations = declarations(
            "/// - value (int | none): what it takes.\n\
             /// - label (string): how it is named.\n\
             /// -> array\n\
             #let sample(value: 1, label: \"x\") = (value,)",
        );
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
        let declarations =
            declarations("/// - missing (int): nowhere.\n#let sample(value) = value");
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
        let declarations =
            declarations("/// Address name.\n#import \"@tola/host:0.0.0\": slugify as name");
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
        let declarations = declarations(
            "#let original(first, fallback: none) = first\n\
             #let alias = original\n\
             #let original(second) = second\n\
             #let original = original",
        );
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
