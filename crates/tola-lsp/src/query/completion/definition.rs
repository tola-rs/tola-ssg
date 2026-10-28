//! What a name's definition says about its completion: the kind, the declared signature, and the
//! documentation above the declaration.

use std::collections::BTreeMap;
use std::ops::Range;

use anyhow::Result;
use lsp_types::{CompletionItem, CompletionItemKind};
use tola_build::cancellation::BuildCancellation;
use tola_packages::{BuiltinPackage, ExportDeclaration, ExportDocumentation, Signature};
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast;
use tola_typst::typst::syntax::{LinkedNode, Source, SyntaxKind, VirtualRoot};
use tola_typst_syntax::docs::{Documentation, declaration_docs};
use tola_typst_syntax::names::{Declaration, DeclarationKind, Initializer, SourceNames};

use super::super::expression::ExpressionType;
use super::super::semantic;
use super::callable::{Callable, Receiver};
use super::{labelled_type, markdown_documentation, unparenthesized};

/// The kind a declaration promises: a closure is a function, an import of a whole module is a
/// module, every other declaration names a value.
pub(super) fn declaration_kind(initializer: &Initializer) -> CompletionItemKind {
    match initializer {
        Initializer::Function { .. } => CompletionItemKind::FUNCTION,
        Initializer::Import { path, .. } if path.is_empty() => CompletionItemKind::MODULE,
        _ => CompletionItemKind::VALUE,
    }
}

/// The kind and the signature beside the label, for a callable a completion offers.
pub(super) fn attach_signature(item: &mut CompletionItem, signature: &Signature) {
    item.kind = Some(CompletionItemKind::FUNCTION);
    item.label_details = Some(labelled_type(Some(semantic::inline_signature(signature))));
}

/// What a known value says about the name holding it: kind, type description, and its own
/// documentation. A function value also declares the call to insert.
pub(super) fn attach_value(item: &mut CompletionItem, value: Value) -> Option<Callable> {
    let edit = item.text_edit.clone()?;
    let callable = Callable::of_value(&value, Receiver::Unbound);
    let completed = semantic::completion(&item.label, &[ExpressionType::Known(value)], edit);
    item.kind = completed.kind;
    item.label_details = completed.label_details;
    item.documentation = completed.documentation;
    callable
}

/// The package this site ships that publishes one name, whose own export documentation then
/// owns it: no import or alias the file writes can change what that package publishes.
pub(super) fn exporting_package(names: &SourceNames, name: &str) -> Option<BuiltinPackage> {
    let id = names.source().id();
    let VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    let package = tola_packages::builtin_package(spec)?;
    package
        .exports()
        .any(|export| export == name)
        .then_some(package)
}

/// Every package export's documentation one completion has read, so a package this site ships is
/// read once however many of its names complete.
#[derive(Default)]
pub(super) struct PackageExports(BTreeMap<String, Vec<ExportDocumentation>>);

impl PackageExports {
    /// The documentation a package publishes for its exports, read on first use.
    fn documentation(&mut self, package: &BuiltinPackage) -> &[ExportDocumentation] {
        self.0.entry(package.spec().to_string()).or_insert_with(|| {
            let names = package.exports().map(str::to_owned).collect::<Vec<_>>();
            package.export_documentation(&names).unwrap_or_default()
        })
    }
}

/// What one declaration in place says about its name's item: a package this site ships documents
/// its own exports, and every other declaration is described by its own text.
pub(super) fn attach_declaration(
    item: &mut CompletionItem,
    names: &SourceNames,
    declaration: &Declaration,
    packages: &mut PackageExports,
    cancellation: &BuildCancellation,
) -> Result<Option<Callable>> {
    cancellation.ensure_active()?;
    if !matches!(
        declaration.kind,
        DeclarationKind::Let | DeclarationKind::Import { .. }
    ) {
        return Ok(None);
    }
    match exporting_package(names, &declaration.name) {
        Some(package) => attach_package_export(item, declaration, package, packages, cancellation),
        None => Ok(attach_own_text(item, names, declaration)),
    }
}

/// The export documentation a package this site ships writes for one exported name.
fn attach_package_export(
    item: &mut CompletionItem,
    declaration: &Declaration,
    package: BuiltinPackage,
    packages: &mut PackageExports,
    cancellation: &BuildCancellation,
) -> Result<Option<Callable>> {
    let exports = packages.documentation(&package);
    cancellation.ensure_active()?;
    let Some(export) = exports
        .iter()
        .find(|export| export.name == declaration.name)
    else {
        return Ok(None);
    };
    let shape = match &export.declaration {
        ExportDeclaration::Function(signature) => {
            let mut signature = signature.clone();
            signature.name.clone_from(&item.label);
            attach_signature(item, &signature);
            Some(Callable::of_signature(&signature))
        }
        ExportDeclaration::Value(value) => {
            item.label_details = Some(labelled_type(Some(value.clone())));
            None
        }
    };
    if !export.documentation.summary.is_empty() {
        item.documentation = Some(markdown_documentation(&export.documentation.summary));
    }
    Ok(shape)
}

/// What a declaration's own text says: the signature its closure writes, and the documentation
/// above it.
fn attach_own_text(
    item: &mut CompletionItem,
    names: &SourceNames,
    declaration: &Declaration,
) -> Option<Callable> {
    let source = names.source();
    let node = declaration_node(source, &declaration.range)?;
    let documentation = declaration_docs(source, node.range().start);
    let signature = closure_signature(source, node, &item.label, documentation.as_deref());
    if let Some(signature) = &signature {
        attach_signature(item, signature);
    }
    if let Some(documentation) = documentation {
        item.documentation = Some(markdown_documentation(documentation));
    }
    signature.map(|signature| Callable::of_signature(&signature))
}

/// The `let` binding or import statement one declaration's name belongs to.
fn declaration_node<'a>(source: &'a Source, range: &Range<usize>) -> Option<LinkedNode<'a>> {
    let mut node = tola_typst_syntax::syntax::node_at_range(source, range)?;
    while !matches!(
        node.kind(),
        SyntaxKind::LetBinding | SyntaxKind::ModuleImport
    ) {
        node = node.parent()?.clone();
    }
    Some(node)
}

/// The signature the closure of one `let` binding writes, with the documentation above the
/// declaration annotating it. Types and defaults keep the declaration's spelling; nothing is
/// evaluated.
fn closure_signature(
    source: &Source,
    node: LinkedNode<'_>,
    name: &str,
    documentation: Option<&str>,
) -> Option<Signature> {
    let binding = node.cast::<ast::LetBinding>()?;
    // A destructuring pattern binds several names; none of them is the callable on its own.
    if binding.kind().bindings().len() != 1 {
        return None;
    }
    let ast::Expr::Closure(closure) = unparenthesized(binding.init())? else {
        return None;
    };
    Signature::from_source(
        name,
        source,
        &closure,
        &Documentation::parse(documentation.unwrap_or_default()),
    )
}
