//! The source-only analysis lane, and where it hands a query to the compiler lane.

use anyhow::Result;

use super::jobs::{AnalysisContinuation, AnalysisRequest};
use super::worker::{SourceCompilation, SourceFailure};
use crate::protocol::SourceReply;

pub(crate) fn analyze(
    disk: &mut crate::analysis::DiskSources,
    graphs: &mut crate::analysis::GraphCache,
    request: AnalysisRequest,
) -> SourceCompilation {
    let response = (|| -> Result<Option<SourceReply>> {
        request.cancellation.ensure_active()?;
        let names = crate::analysis::source_only_graph(
            &request.root,
            &request.view,
            &request.source,
            request.package_locations.as_ref(),
            &request.boundary,
            true,
            disk,
            Some(crate::analysis::CachedGraph {
                cache: graphs,
                revision: request.source_revision,
            }),
            &request.cancellation,
        )?;
        if !crate::analysis::unproven(&names, &request.source, request.cursor, &request.query)
            .is_empty()
        {
            return Ok(None);
        }
        let response = crate::analysis::respond(
            &names,
            &request.source,
            request.cursor,
            &request.query,
            &request.root,
            request.package_locations.as_ref(),
            &request.boundary,
        )?;
        request.cancellation.ensure_active()?;
        Ok(Some(response.unwrap_or_else(|| {
            crate::query::unavailable(&request.query)
        })))
    })()
    .map_err(SourceFailure::of);
    match response {
        Ok(Some(response)) => SourceCompilation::Answered {
            id: request.id,
            serial: request.serial,
            response: Ok(response),
        },
        Ok(None) => SourceCompilation::Continued(AnalysisContinuation {
            id: request.id,
            serial: request.serial,
            root: request.root,
            view: request.view,
            query: request.query,
            cancellation: request.cancellation,
        }),
        Err(failure) => SourceCompilation::Answered {
            id: request.id,
            serial: request.serial,
            response: Err(failure),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::tests::assert_cancelled;
    use crate::sources::SourceView;
    use lsp_server::RequestId;
    use std::path::Path;
    use tola_build::cancellation::{BuildCancellation, BuildCanceller};

    #[test]
    fn cancelled_analysis_stays_cancelled() {
        let directory = tempfile::tempdir().unwrap();
        let root = tola_build::filesystem::normalize_existing_prefix(directory.path());
        let (mut request, _) = references_analysis(&root, "#let marker = 1\n#mar|ker\n");
        let canceller = BuildCanceller::new();
        canceller.cancel();
        request.cancellation = canceller.token();
        assert_cancelled(analyze(
            &mut crate::analysis::DiskSources::default(),
            &mut crate::analysis::GraphCache::default(),
            request,
        ));
    }

    /// One references analysis over `marked`, whose single `|` is the cursor, and the text the
    /// marker strips to.
    fn references_analysis(root: &Path, marked: &str) -> (AnalysisRequest, String) {
        let cursor = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        let path = root.join("document.typ");
        let uri = crate::uri::from_file_path(&path).unwrap();
        let id = crate::identity::file_id(&uri, root).unwrap();
        let source = tola_typst::typst::syntax::Source::new(id, text.clone());
        let position = crate::position::utf16_range(source.lines(), cursor..cursor)
            .unwrap()
            .start;
        let request = AnalysisRequest {
            id: 1.into(),
            serial: 1,
            boundary: tola_typst::SourceBoundary::new(root, true),
            root: root.to_path_buf(),
            view: SourceView::default(),
            source,
            cursor,
            package_locations: None,
            query: crate::protocol::SourceQuery::References(lsp_types::ReferenceParams {
                text_document_position: lsp_types::TextDocumentPositionParams {
                    text_document: lsp_types::TextDocumentIdentifier::new(uri),
                    position,
                },
                context: lsp_types::ReferenceContext {
                    include_declaration: true,
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            }),
            cancellation: BuildCancellation::new(),
            source_revision: 0,
        };
        (request, text)
    }

    #[test]
    fn dynamic_import_references_defer_to_the_compiler_lane() {
        let directory = tempfile::tempdir().unwrap();
        let root = tola_build::filesystem::normalize_existing_prefix(directory.path());
        std::fs::write(root.join("lib.typ"), "#let greet = [hello]\n").unwrap();
        let (request, _) = references_analysis(
            &root,
            "#let base = \"lib\"\n#import base + \".typ\": greet\n#gre|et\n",
        );
        let SourceCompilation::Continued(continued) = analyze(
            &mut crate::analysis::DiskSources::default(),
            &mut crate::analysis::GraphCache::default(),
            request,
        ) else {
            panic!("the compiler lane owns a dynamic import's references");
        };
        assert_eq!(continued.id, RequestId::from(1));
        assert_eq!(continued.serial, 1);
        assert!(matches!(
            continued.query,
            crate::protocol::SourceQuery::References(_)
        ));
    }

    #[test]
    fn literal_import_references_answer_in_the_source_lane() {
        let directory = tempfile::tempdir().unwrap();
        let root = tola_build::filesystem::normalize_existing_prefix(directory.path());
        std::fs::write(root.join("lib.typ"), "#let greet = [hello]\n").unwrap();
        let (request, text) = references_analysis(&root, "#import \"lib.typ\": greet\n#gre|et\n");
        let SourceCompilation::Answered { response, .. } = analyze(
            &mut crate::analysis::DiskSources::default(),
            &mut crate::analysis::GraphCache::default(),
            request,
        ) else {
            panic!("a literal path needs no compiler lane");
        };
        let Ok(SourceReply::References(Some(found))) = response else {
            panic!("the source lane answers the references");
        };
        let document = crate::uri::from_file_path(&root.join("document.typ")).unwrap();
        let spellings: Vec<_> = found
            .iter()
            .filter(|location| location.uri == document)
            .map(|location| location.range)
            .collect();
        let source = tola_typst::typst::syntax::Source::detached(text.as_str());
        let expected: Vec<_> = text
            .match_indices("greet")
            .map(|(start, name)| {
                crate::position::utf16_range(source.lines(), start..start + name.len()).unwrap()
            })
            .collect();
        assert_eq!(spellings, expected);
    }
}
