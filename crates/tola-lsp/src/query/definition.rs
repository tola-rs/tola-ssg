//! Where one name is defined.

use anyhow::Result;
use lsp_types::GotoDefinitionResponse;
use tola_typst::typst::syntax::{Source, SyntaxKind};

use super::context::{SelectedSyntax, Selection};
use super::semantic::Semantic;

/// How many candidate sources one answer names.
const CANDIDATE_LIMIT: usize = 20;

impl Selection<'_> {
    pub(super) fn definition(
        &self,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
        client_root: &crate::uri::ClientRoot,
    ) -> Result<Option<GotoDefinitionResponse>> {
        if let SelectedSyntax::ImportedName(item) = &self.syntax {
            let Some(module) = tola_typst_syntax::syntax::node_at_range(prepared, &item.source)
            else {
                return Ok(None);
            };
            return Ok(semantics
                .imported_location(&module, &item.path, client_root)?
                .map(GotoDefinitionResponse::Scalar));
        }
        let Some(node) = tola_typst_syntax::syntax::expression(prepared, self.cursor) else {
            return Ok(None);
        };
        // A node the query copy repaired is a stand-in: no definition answers about it.
        if !self.range_keeps_source_text(prepared, node.range()) {
            return Ok(None);
        }
        // A chain reading site sources jumps to the files the site observed its records in: every
        // candidate the chain was realized with, each at the binding that file declares.
        if matches!(
            node.kind(),
            SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess
        ) {
            let candidates = semantics.candidate_sources(&node)?;
            let locations = candidates
                .iter()
                .take(CANDIDATE_LIMIT)
                .filter_map(|candidate| semantics.candidate_location(candidate, client_root))
                .collect::<Vec<_>>();
            if !locations.is_empty() {
                return Ok(Some(GotoDefinitionResponse::Array(locations)));
            }
        }
        Ok(semantics
            .definition_location(&node, client_root)?
            .map(GotoDefinitionResponse::Scalar))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position;
    use crate::query::package_source;
    use crate::query::tests::*;
    use serde_json::json;

    /// A reference the query copy repaired is a stand-in: no definition answers about the
    /// enclosing construct it left behind.
    #[test]
    fn repaired_reference_answers_no_definition() {
        let mut site = QuerySession::new();
        let definition = site.definition("#let f(..xs) = xs.len()\n#f(<nope|)\n");
        assert!(definition.is_none(), "{definition:?}");
    }

    #[test]
    fn definitions_point_at_package_sources() {
        let mut site = QuerySession::new();
        let definition = site.reply(
            "textDocument/definition",
            r#"#import "@tola/document:0.0.0": current-document as identity
    #let value = ident|ity
    Body"#,
        );
        let uri = definition["uri"].as_str().unwrap();
        assert_eq!(uri, "tola-package:/tola/document/0.0.0/lib.typ");
        let uri: lsp_types::Uri = uri.parse().unwrap();
        let virtual_source = package_source(&uri).unwrap();
        let id = crate::identity::file_id(&uri, site.config.get_root()).unwrap();
        assert_eq!(
            virtual_source.text,
            crate::identity::embedded_source(id).unwrap()
        );
        let source = Source::new(id, virtual_source.text);
        let range: lsp_types::Range = serde_json::from_value(definition["range"].clone()).unwrap();
        let start = position::byte_offset(source.lines(), range.start).unwrap();
        let end = position::byte_offset(source.lines(), range.end).unwrap();
        assert_eq!(&source.text()[start..end], "current-document");

        let import = site.reply(
            "textDocument/definition",
            r#"#import "@tola/doc|ument:0.0.0": current-document
    Body"#,
        );
        assert_eq!(import["uri"], uri.as_str());
        assert_eq!(
            import["range"]["start"],
            json!({ "line": 0, "character": 0 })
        );
    }

    #[test]
    fn internal_package_imports_resolve() {
        let mut site = QuerySession::new();
        let definition = site.reply(
            "textDocument/definition",
            "#import \"@tola/ho|st:0.0.0\": all-sources\nBody",
        );
        assert_eq!(definition["uri"], "tola-package:/tola/host/0.0.0/lib.typ");
    }
    /// A program the site cannot compile still answers a definition inside it.
    #[test]
    fn broken_programs_still_answer_definitions() {
        let mut site = QuerySession::new();
        let definition = site.reply(
            "textDocument/definition",
            "#import \"@tola/doc|ument:0.0.0\": current-document\n#unknown_after_import",
        );
        assert_eq!(
            definition["uri"],
            "tola-package:/tola/document/0.0.0/lib.typ"
        );
    }

    /// A metadata chain jumps to the file its records came from, at the binding that file declares.
    #[test]
    fn metadata_chain_jumps_to_its_source_file() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let Some(GotoDefinitionResponse::Array(locations)) =
            site_definition(&mut site, &marked_before(&program, "aft)"))
        else {
            panic!("expected the candidate locations");
        };
        assert_eq!(locations.len(), 1, "{locations:?}");
        assert!(
            locations[0].uri.as_str().ends_with("/content/page.typ"),
            "{:?}",
            locations[0].uri
        );
        let source = Source::detached(SCHEMA_PAGE.to_owned());
        let start = position::byte_offset(source.lines(), locations[0].range.start).unwrap();
        let end = position::byte_offset(source.lines(), locations[0].range.end).unwrap();
        assert_eq!(&source.text()[start..end], "page");
    }
    #[test]
    fn chain_without_file_answers_no_definition() {
        let mut site = QuerySession::new();
        let definition = site.definition("#let author = (name: \"A\")\n#author.na|me\n");
        assert!(definition.is_none(), "{definition:?}");
    }
    /// A binding inside parentheses answers like one beside its use: parentheses open no scope.
    #[test]
    fn grouped_bindings_answer_definitions() {
        for marked in ["#((((let y = 1))));\n#(y|);\n", "#(let (x) = 2);\n#(x|);\n"] {
            let mut site = QuerySession::new();
            let Some(GotoDefinitionResponse::Scalar(location)) = site.definition(marked) else {
                panic!("no definition for {marked:?}");
            };
            assert_eq!(location.range.start.line, 0);
        }
    }

    /// A member of an imported module answers the definition inside that module.
    #[test]
    fn imported_members_answer_definitions() {
        let mut site = QuerySession::new();
        site.site
            .write("content/helpers.typ", "#let helper(value) = value\n");
        let Some(GotoDefinitionResponse::Scalar(location)) =
            site.definition("#import \"helpers.typ\"\n#(helpers.hel|per(1))\n")
        else {
            panic!("no definition for an imported module member");
        };
        assert!(
            location.uri.as_str().ends_with("content/helpers.typ"),
            "{location:?}"
        );
    }
}
