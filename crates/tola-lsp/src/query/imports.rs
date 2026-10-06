//! What this file's imports establish about one callee: the import item a name selects.

use tola_typst::typst::syntax::LinkedNode;
use tola_typst::typst::syntax::ast::{self, AstNode};

/// One import item a callee names, as the file establishes it: the statement whose module
/// interface the path is read from, and the path itself.
pub(super) struct ImportedMember<'n> {
    /// The statement that supplies the module interface.
    pub(super) import: &'n tola_typst_syntax::names::Import,
    /// The external path the callee selects, in the module's own spelling.
    pub(super) path: Vec<String>,
}

/// The import item one callee names, when this file establishes one.
///
/// The identity is source-established: a named import of the item, with or without an alias, or a
/// field read on the module imported whole. A wildcard import loads no interface here, so it names
/// nothing this file can establish.
pub(super) fn imported_member<'n>(
    names: &'n tola_typst_syntax::names::SourceNames,
    callee: &LinkedNode<'_>,
) -> Option<ImportedMember<'n>> {
    let (declaration, field) = match callee.cast::<ast::FieldAccess>() {
        Some(access) => {
            let target = callee.find(access.target().span())?;
            (
                names.declared_at(target.range().start)?,
                Some(access.field().get().to_string()),
            )
        }
        None => (names.declared_at(callee.range().start)?, None),
    };
    let declaration = &names.declarations()[declaration];
    let tola_typst_syntax::names::Initializer::Import { import, path } = &declaration.initializer
    else {
        return None;
    };
    let path = match field {
        Some(field) => path.is_empty().then(|| vec![field])?,
        None => (path.len() == 1).then(|| path.clone())?,
    };
    Some(ImportedMember {
        import: &names.imports()[*import],
        path,
    })
}

/// Whether a callee names one item of a package this file establishes by import.
pub(super) fn calls_package_member(
    names: &tola_typst_syntax::names::SourceNames,
    callee: &LinkedNode<'_>,
    package: &str,
    item: &str,
) -> bool {
    let Some(imported) = imported_member(names, callee) else {
        return false;
    };
    let tola_typst_syntax::names::ImportSource::Path(source) = &imported.import.source else {
        return false;
    };
    imported.path == [item] && is_package(source, package)
}

/// Whether an import path names one package of the `@tola` namespace.
fn is_package(source: &str, name: &str) -> bool {
    source
        .parse::<tola_typst::typst::syntax::package::PackageSpec>()
        .is_ok_and(|spec| {
            spec.namespace.as_str() == tola_packages::TOLA_NAMESPACE && spec.name.as_str() == name
        })
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;
    use lsp_types::GotoDefinitionResponse;

    #[test]
    fn import_path_hover_names_the_file() {
        let mut site = QuerySession::new();
        site.site.write("content/head.typ", "#let head = [Head]\n");

        let hover = site
            .hover_text("#import \"hea|d.typ\": head\nBody")
            .expect("a hover for the path");
        assert!(hover.contains("content/head.typ"), "{hover}");
        assert!(hover.contains("Served at"), "{hover}");
    }

    /// An import item's name answers wherever the export lives, not only inside a Tola package.
    #[test]
    fn imported_name_hover_names_its_file() {
        let mut site = QuerySession::new();
        site.site
            .write("content/library.typ", "#let helper() = 1\n");

        let hover = site
            .hover_text("#import \"library.typ\": hel|per\nBody")
            .expect("a hover for the imported name");
        assert!(hover.contains("content/library.typ"), "{hover}");
    }
    /// An import item's own name answers a cursor question whether or not the item renames it:
    /// the name is not a binding in this file, so nothing else can resolve it.
    #[test]
    fn import_items_answer_at_their_own_name() {
        let mut site = QuerySession::new();
        // The expected summary comes from the package's own documentation, so the test does not
        // pin a sentence the package source is free to rewrite.
        let summary = tola_packages::builtin_package(&tola_packages::TolaPackage::Document.spec())
            .expect("the builtin `@tola/document`")
            .export_documentation(&["current-document".to_owned()])
            .expect("the export's documentation")
            .remove(0)
            .documentation
            .summary;
        assert!(
            !summary.is_empty(),
            "`current-document` names package documentation"
        );
        for marked in [
            "#import \"@tola/document:0.0.0\": current-docu|ment\nBody",
            "#import \"@tola/document:0.0.0\": current-docu|ment as identity\nBody",
        ] {
            let hover = site.hover_text(marked).expect("a hover");
            assert!(hover.contains("current-document("), "{hover}");
            assert!(hover.contains(&summary), "{hover}");
            let Some(GotoDefinitionResponse::Scalar(location)) = site.definition(marked) else {
                panic!("expected one package location");
            };
            assert_eq!(
                location.uri.as_str(),
                "tola-package:/tola/document/0.0.0/lib.typ"
            );
        }
    }
}
