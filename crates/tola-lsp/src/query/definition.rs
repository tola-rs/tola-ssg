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
        prep: &Source,
        sem: &mut Semantic<'_>,
        client_root: &crate::uri::ClientRoot,
    ) -> Result<Option<GotoDefinitionResponse>> {
        if let SelectedSyntax::ImportedName(item) = &self.syntax {
            let Some(module) = tola_typst_syntax::syntax::node_at_range(prep, &item.source) else {
                return Ok(None);
            };
            return Ok(sem
                .imported_location(&module, &item.path, client_root)?
                .map(GotoDefinitionResponse::Scalar));
        }
        let Some(node) = tola_typst_syntax::syntax::expression(prep, self.cursor) else {
            return Ok(None);
        };
        if !self.range_keeps_source_text(prep, node.range()) {
            return Ok(None);
        }
        if matches!(
            node.kind(),
            SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess
        ) {
            let locs = sem
                .candidate_sources(&node)?
                .iter()
                .take(CANDIDATE_LIMIT)
                .filter_map(|c| sem.candidate_location(c, client_root))
                .collect::<Vec<_>>();
            if !locs.is_empty() {
                return Ok(Some(GotoDefinitionResponse::Array(locs)));
            }
        }
        Ok(sem
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

    #[test]
    #[cfg(unix)]
    fn symlink_module_relative_imports() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/real/library.typ",
            "#import \"helper.typ\": value\n",
        );
        site.site
            .write("content/real/helper.typ", "#let value = 1\n");
        site.site
            .write("content/alias/helper.typ", "#let value = 2\n");
        std::os::unix::fs::symlink(
            "../real/library.typ",
            site.site.root().join("content/alias/library.typ"),
        )
        .unwrap();
        let Some(GotoDefinitionResponse::Scalar(loc)) =
            site.definition("#import \"alias/library.typ\": value\n#val|ue\n")
        else {
            panic!("the re-export has a definition");
        };
        assert!(
            loc.uri.as_str().ends_with("content/alias/helper.typ"),
            "{loc:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn symlink_imports_dedup_references() {
        let mut site = QuerySession::new();
        site.site.write("content/library.typ", "#let value = 1\n");
        std::os::unix::fs::symlink("library.typ", site.site.root().join("content/alias.typ"))
            .unwrap();
        site.site.write(
            "content/document.typ",
            "#import \"alias.typ\": value\n#value\n",
        );
        site.site.write(
            "content/consumer.typ",
            "#import \"library.typ\": value\n#value\n",
        );
        let uri =
            crate::uri::from_file_path(&site.site.root().join("content/library.typ")).unwrap();
        let reply = site
            .try_reply_at(
                "textDocument/references",
                uri.as_str(),
                lsp_types::Position::new(0, 6),
                None,
                json!({"context":{"includeDeclaration":false}}),
            )
            .unwrap();
        let refs: Vec<lsp_types::Location> = serde_json::from_value(reply).unwrap();
        assert_eq!(refs.len(), 2, "{refs:?}");
    }

    #[test]
    fn repaired_reference_no_definition() {
        let mut site = QuerySession::new();
        let def = site.definition("#let f(..xs) = xs.len()\n#f(<nope|)\n");
        assert!(def.is_none(), "{def:?}");
    }

    #[test]
    fn package_definitions_address_source() {
        let mut site = QuerySession::new();
        let alias_use = r#"#import "@tola/document:0.0.0": current-document as identity
    #let value = ident|ity
    Body"#;
        for (marked, expected) in [
            (alias_use, "tola-package:/tola/document/0.0.0/lib.typ"),
            (
                r#"#import "@tola/doc|ument:0.0.0": current-document
    Body"#,
                "tola-package:/tola/document/0.0.0/lib.typ",
            ),
            (
                r#"#import "@tola/ho|st:0.0.0": all-sources
    Body"#,
                "tola-package:/tola/host/0.0.0/lib.typ",
            ),
        ] {
            let def = site.reply("textDocument/definition", marked);
            assert_eq!(def["uri"], expected, "{marked:?}");
        }

        let def = site.reply("textDocument/definition", alias_use);
        let uri: lsp_types::Uri = def["uri"].as_str().unwrap().parse().unwrap();
        let virtual_src = package_source(&uri).unwrap();
        let id = crate::identity::file_id(&uri, site.config.root()).unwrap();
        assert_eq!(
            virtual_src.text,
            crate::identity::embedded_source(id).unwrap()
        );
        let src = Source::new(id, virtual_src.text);
        let range: lsp_types::Range = serde_json::from_value(def["range"].clone()).unwrap();
        let start = position::byte_offset(src.lines(), range.start).unwrap();
        let end = position::byte_offset(src.lines(), range.end).unwrap();
        assert_eq!(&src.text()[start..end], "current-document");

        let import = site.reply(
            "textDocument/definition",
            r#"#import "@tola/doc|ument:0.0.0": current-document
    Body"#,
        );
        assert_eq!(
            import["range"]["start"],
            json!({ "line": 0, "character": 0 })
        );
    }

    #[test]
    fn broken_site_definition() {
        let mut site = QuerySession::new();
        let def = site.reply(
            "textDocument/definition",
            "#import \"@tola/doc|ument:0.0.0\": current-document\n#unknown_after_import",
        );
        assert_eq!(def["uri"], "tola-package:/tola/document/0.0.0/lib.typ");
    }

    #[test]
    fn metadata_chain_definition() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let Some(GotoDefinitionResponse::Array(locs)) =
            site_definition(&mut site, &marked_before(&program, "aft)"))
        else {
            panic!("expected the candidate locations");
        };
        assert_eq!(locs.len(), 1, "{locs:?}");
        assert!(
            locs[0].uri.as_str().ends_with("/content/page.typ"),
            "{:?}",
            locs[0].uri
        );
        let src = Source::detached(SCHEMA_PAGE.to_owned());
        let start = position::byte_offset(src.lines(), locs[0].range.start).unwrap();
        let end = position::byte_offset(src.lines(), locs[0].range.end).unwrap();
        assert_eq!(&src.text()[start..end], "page");

        let plain = QuerySession::new().definition("#let author = (name: \"A\")\n#author.na|me\n");
        assert!(plain.is_none(), "{plain:?}");
    }

    #[test]
    fn imported_member_definition() {
        let mut site = QuerySession::new();
        site.site
            .write("content/helpers.typ", "#let helper(value) = value\n");
        let Some(GotoDefinitionResponse::Scalar(loc)) =
            site.definition("#import \"helpers.typ\"\n#(helpers.hel|per(1))\n")
        else {
            panic!("no definition for an imported module member");
        };
        assert!(loc.uri.as_str().ends_with("content/helpers.typ"), "{loc:?}");
    }
}
