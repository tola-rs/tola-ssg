//! Static AST resolution of bundled package exports and documentation blocks.

use std::path::Path;

use tola_typst_syntax::docs::Documentation;
use typst::foundations::{Func, Scope, Value};
use typst::syntax::ast::AstNode;
use typst::syntax::{LinkedNode, Source, Span, SyntaxKind, ast};

use super::declaration::{ExportDocumentation, Signature};
use super::related;
use super::{ExportDocumentationError, RelatedTarget};
use ExportDocumentationError::{
    InvalidDeclaration, InvalidImport, MemberOutsideDictionary, MissingBinding, MissingDictionary,
    MissingMember, MissingSource, UnknownNative,
};

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

fn find_node<'a>(
    file: &'a FileSource,
    span: Span,
    msg: &'static str,
) -> Result<LinkedNode<'a>, ExportDocumentationError> {
    file.1.find(span).ok_or(InvalidDeclaration(msg))
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
                for stmt in parent.children().rev() {
                    // Initializers see preceding bindings (including shadowed outer bindings).
                    if before.is_some_and(|b| stmt.range().end > b) {
                        continue;
                    }
                    if let Some(binding) = stmt.cast::<ast::LetBinding>() {
                        if !binding.kind().bindings().iter().any(|b| b.as_str() == name) {
                            continue;
                        }
                        let doc = declared_documentation(&file.1, stmt.range().start);
                        let init = binding
                            .init()
                            .ok_or(InvalidDeclaration("bundled binding has no initializer"))?;
                        let node =
                            find_node(file, init.span(), "bundled initializer has no source")?;
                        let (decl, inherited) = self.expression(file, node)?;
                        return Ok((decl, doc.or(inherited)));
                    }
                    let Some(import) = stmt.cast::<ast::ModuleImport>() else {
                        continue;
                    };
                    let Some(ast::Imports::Items(items)) = import.imports() else {
                        continue;
                    };
                    if let Some(item) = items.iter().find(|i| i.bound_name().as_str() == name) {
                        return self.resolve_import(file, &stmt, &import, item);
                    }
                }
            }
            before = Some(parent.range().start);
            scope = parent.parent().cloned();
        }
        Err(MissingBinding(name.to_owned()))
    }

    fn resolve_import<'a>(
        &'a self,
        file: &'a FileSource,
        stmt: &LinkedNode<'a>,
        import: &ast::ModuleImport<'_>,
        item: ast::ImportItem<'_>,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>), ExportDocumentationError> {
        let doc = declared_documentation(&file.1, stmt.range().start);
        let ast::Expr::Str(path) = import.source() else {
            return Err(InvalidDeclaration("bundled import is not a literal path"));
        };
        let path = path.get();
        let original = item.original_name();
        if path.as_str() == "@tola/host:0.0.0" {
            if original.as_str() == "site" {
                return Ok((Declaration::Site, doc));
            }
            let val = self
                .natives
                .get(original.as_str())
                .ok_or_else(|| UnknownNative(original.as_str().to_owned()))?
                .read();
            let decl = match val {
                Value::Func(func) => Declaration::Native(func.clone()),
                val => Declaration::Constant(val.clone()),
            };
            return Ok((decl, doc));
        }
        if path.starts_with("@tola/") {
            let spec = path
                .parse::<tola_typst::PackageSpec>()
                .map_err(|err| InvalidImport(err.to_string()))?;
            let pkg = crate::builtin_package(&spec)
                .ok_or(InvalidDeclaration("unknown bundled package import"))?;
            let mut exports = pkg.export_documentation(&[original.as_str().to_owned()])?;
            let export = exports
                .pop()
                .ok_or(InvalidDeclaration("bundled import has no declaration"))?;
            let inherited = DeclaredDocumentation {
                documentation: export.documentation.clone(),
                related: export.related.clone(),
            };
            return Ok((Declaration::Export(export), doc.or(Some(inherited))));
        }
        let base = Path::new(file.0).parent().unwrap_or_else(|| Path::new(""));
        let imported = self.file(
            base.join(path.as_str())
                .to_str()
                .ok_or(InvalidDeclaration("builtin import path is not UTF-8"))?,
        )?;
        let (decl, inherited) = self.binding(
            imported,
            LinkedNode::new(imported.1.root()),
            original.as_str(),
        )?;
        Ok((decl, doc.or(inherited)))
    }

    fn expression<'a>(
        &'a self,
        file: &'a FileSource,
        node: LinkedNode<'a>,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>), ExportDocumentationError> {
        let Some(expr) = node.cast::<ast::Expr>() else {
            return Err(InvalidDeclaration(
                "bundled declaration is not an expression",
            ));
        };
        match expr {
            ast::Expr::Ident(ident) => self.binding(file, node.clone(), ident.as_str()),
            ast::Expr::FieldAccess(access) => {
                let target = find_node(file, access.target().span(), "field target has no source")?;
                let (target, _) = self.expression(file, target)?;
                self.member(target, access.field().as_str())
            }
            ast::Expr::CodeBlock(block) => {
                let last = block
                    .body()
                    .exprs()
                    .next_back()
                    .ok_or(InvalidDeclaration("bundled block has no final expression"))?;
                let node = find_node(file, last.span(), "block result has no source")?;
                self.expression(file, node)
            }
            ast::Expr::Parenthesized(group) => {
                let node = find_node(file, group.expr().span(), "group has no source")?;
                self.expression(file, node)
            }
            _ => Ok((Declaration::Source(file, node), None)),
        }
    }

    fn member<'a>(
        &'a self,
        decl: Declaration<'a>,
        name: &str,
    ) -> Result<(Declaration<'a>, Option<DeclaredDocumentation>), ExportDocumentationError> {
        let Declaration::Source(file, node) = decl else {
            return Err(MissingDictionary(name.to_owned()));
        };
        let expr = node
            .cast::<ast::Expr>()
            .ok_or(InvalidDeclaration("dictionary is not an expression"))?;
        match expr {
            ast::Expr::Dict(dict) => {
                for item in dict.items().rev() {
                    match item {
                        ast::DictItem::Named(named) if named.name().as_str() == name => {
                            let node = find_node(
                                file,
                                named.expr().span(),
                                "dictionary member has no source",
                            )?;
                            return self.expression(file, node);
                        }
                        ast::DictItem::Spread(spread) => {
                            let node =
                                find_node(file, spread.expr().span(), "spread has no source")?;
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
                let callee_node = find_node(file, call.callee().span(), "callee has no source")?;
                let (callee, _) = self.expression(file, callee_node)?;
                let Declaration::Source(file, node) = callee else {
                    return Err(InvalidDeclaration(
                        "bundled dictionary factory is not a source closure",
                    ));
                };
                let closure = node.cast::<ast::Closure>().ok_or(InvalidDeclaration(
                    "bundled dictionary factory is not a closure",
                ))?;
                let body = find_node(file, closure.body().span(), "factory result has no source")?;
                let (returned, _) = self.expression(file, body)?;
                self.member(returned, name)
            }
            _ => Err(MemberOutsideDictionary(name.to_owned())),
        }
    }
}

pub(super) fn declared_documentation(src: &Source, start: usize) -> Option<DeclaredDocumentation> {
    let block = tola_typst_syntax::docs::declaration_docs(src, start)?;
    let related = related::read(&block);
    Some(DeclaredDocumentation {
        documentation: Documentation::parse(&related.prose),
        related: related.targets,
    })
}

pub(super) fn source_function(
    name: &str,
    file: &FileSource,
    closure: &ast::Closure<'_>,
    docs: &Documentation,
) -> Result<Signature, ExportDocumentationError> {
    let sig = Signature::from_source(name, &file.1, closure, docs)
        .ok_or(InvalidDeclaration("bundled source is absent"))?;
    for doc_param in &docs.parameters {
        if !sig.parameters.iter().any(|p| p.name == doc_param.name) {
            return Err(ExportDocumentationError::UnknownParameter {
                name: name.to_owned(),
                parameter: doc_param.name.clone(),
            });
        }
    }
    Ok(sig)
}

#[cfg(test)]
mod tests {
    use super::super::declaration::Parameter;
    use super::*;

    fn declarations(src: &str) -> Declarations {
        Declarations {
            sources: vec![("lib.typ", Source::detached(src))],
            natives: crate::library::native_scope(),
        }
    }

    fn param<'a>(sig: &'a Signature, name: &str) -> &'a Parameter {
        sig.parameters
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("missing parameter `{name}`"))
    }

    #[test]
    fn doc_types_attach_to_fn() {
        let decls = declarations(
            "/// - value (int | none): what it takes.\n\
             /// - label (string): how it is named.\n\
             /// -> array\n\
             #let sample(value: 1, label: \"x\") = (value,)",
        );
        let file = decls.file("lib.typ").unwrap();
        let (decl, docs) = decls
            .binding(file, LinkedNode::new(file.1.root()), "sample")
            .unwrap();
        let Declaration::Source(file, node) = decl else {
            panic!("sample should be a source closure");
        };
        let closure = node.cast::<ast::Closure>().unwrap();
        let docs = docs.unwrap().documentation;
        let sig = source_function("sample", file, &closure, &docs).unwrap();
        assert_eq!(param(&sig, "value").ty.as_deref(), Some("int | none"));
        assert_eq!(param(&sig, "value").docs.as_deref(), Some("what it takes."));
        assert_eq!(param(&sig, "value").default.as_deref(), Some("1"));
        assert_eq!(param(&sig, "label").ty.as_deref(), Some("string"));
        assert_eq!(sig.returns.as_deref(), Some("array"));
        assert!(docs.summary.is_empty());
    }

    #[test]
    fn doc_unknown_param_rejected() {
        let decls = declarations("/// - missing (int): nowhere.\n#let sample(value) = value");
        let file = decls.file("lib.typ").unwrap();
        let (decl, docs) = decls
            .binding(file, LinkedNode::new(file.1.root()), "sample")
            .unwrap();
        let Declaration::Source(file, node) = decl else {
            panic!("sample should be a source closure");
        };
        let closure = node.cast::<ast::Closure>().unwrap();
        let err =
            source_function("sample", file, &closure, &docs.unwrap().documentation).unwrap_err();
        assert!(err.to_string().contains("`missing`"), "{err}");
    }

    #[test]
    fn import_alias_keeps_doc() {
        let decls =
            declarations("/// Address name.\n#import \"@tola/host:0.0.0\": slugify as name");
        let file = decls.file("lib.typ").unwrap();
        let (decl, docs) = decls
            .binding(file, LinkedNode::new(file.1.root()), "name")
            .unwrap();
        assert_eq!(docs.unwrap().documentation.summary, "Address name.");
        let Declaration::Native(func) = decl else {
            panic!("alias should retain the native function");
        };
        assert!(!Signature::of("name", &func).parameters.is_empty());
    }

    #[test]
    fn let_alias_lexical_scope() {
        let decls = declarations(
            "#let original(first, fallback: none) = first\n\
             #let alias = original\n\
             #let original(second) = second\n\
             #let original = original",
        );
        let file = decls.file("lib.typ").unwrap();
        for (name, expected) in [("alias", "first, fallback: none"), ("original", "second")] {
            let (decl, _) = decls
                .binding(file, LinkedNode::new(file.1.root()), name)
                .unwrap();
            let Declaration::Source(_, node) = decl else {
                panic!("{name} should refer to its preceding source closure");
            };
            let closure = node.cast::<ast::Closure>().unwrap();
            let params = file.1.find(closure.params().span()).unwrap();
            assert_eq!(&file.1.text()[params.range()], format!("({expected})"));
        }
    }
}
