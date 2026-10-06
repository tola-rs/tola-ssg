//! The actions one record answer offers: narrowing a collection to one source.

use anyhow::Result;
use lsp_types::CodeActionOrCommand;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::Source;

use super::context::Selection;
use super::semantic::Semantic;

impl Selection<'_> {
    /// The narrowing actions one metadata chain inside a collection closure justifies.
    ///
    /// The checked world names the files the chain's records came from; a narrowed collection
    /// keeps only the records whose own path is the chosen file.
    pub(super) fn narrowing(
        &self,
        uri: &lsp_types::Uri,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Vec<CodeActionOrCommand>>> {
        let Some(narrowing) = crate::code_actions::narrowing_closure(self.source, self.cursor)
        else {
            return Ok(None);
        };
        let Some(receiver) =
            tola_typst_syntax::syntax::node_at_range(prepared, &narrowing.receiver)
        else {
            return Ok(None);
        };
        let receivers = semantics.trace(receiver.span())?;
        if receivers.is_empty()
            || receivers.len() >= tola_typst::typst::engine::Sink::MAX_VALUES
            || !receivers
                .iter()
                .all(|(value, _)| matches!(value, Value::Array(_)))
        {
            return Ok(None);
        }
        let Some(node) = tola_typst_syntax::syntax::expression(prepared, self.cursor) else {
            return Ok(None);
        };
        let sources = semantics
            .candidate_sources(&node)?
            .iter()
            .filter_map(|candidate| {
                Some(crate::code_actions::NarrowingSource {
                    site_path: candidate.site_path.clone(),
                    content_path: candidate.content_path.clone()?,
                })
            })
            .collect::<Vec<_>>();
        let actions =
            crate::code_actions::narrowing_actions(uri, self.source, &narrowing, &sources);
        Ok((!actions.is_empty()).then_some(actions))
    }
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;

    /// A narrowing action keeps the source it names, joining the predicate the author wrote.
    #[test]
    fn narrowing_action_keeps_one_source() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/a.typ", SCHEMA_PAGE);
        site.site.write("content/b.typ", SCHEMA_PAGE);
        let actions = site_actions(
            &mut site,
            &marked_before(&program, "aft)"),
            &["refactor.rewrite"],
        );
        assert_eq!(actions.len(), 2, "{actions:?}");
        assert_eq!(actions[0]["title"], "restrict to `content/a.typ`");
        assert_eq!(actions[0]["kind"], "refactor.rewrite");
        let inserted = actions[0]["edit"]["changes"]
            .as_object()
            .and_then(|changes| changes.values().next())
            .and_then(|edits| edits.get(0))
            .and_then(|edit| edit["newText"].as_str())
            .expect("one inserted condition");
        assert_eq!(
            inserted,
            "source.path == \"a.typ\" and (not source.meta.draft)"
        );
        assert_eq!(actions[1]["title"], "restrict to `content/b.typ`");
    }

    /// A metadata chain outside a filter closure offers no narrowing action.
    #[test]
    fn metadata_chain_outside_filter_offers_no_action() {
        let program = parsed_program(PAGE_SCHEMA).replace(
            "#let kept = declared.filter(source => not source.meta.draft)",
            "#let kept = declared\n#let draft = declared.first().meta.draft",
        );
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let actions = site_actions(
            &mut site,
            &marked_before(&program, "ta.draft\n"),
            &["refactor.rewrite"],
        );
        assert!(actions.is_empty(), "{actions:?}");
    }
}
