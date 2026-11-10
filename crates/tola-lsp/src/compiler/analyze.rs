//! The source-only analysis lane, and where it hands a query to the compiler lane.

use anyhow::Result;

use super::jobs::{AnalysisContinuation, AnalysisRequest, AnalysisTask, TokensKind, TokensRead};
use super::worker::{SourceCompilation, SourceFailure};
use crate::protocol::SourceReply;

pub(crate) fn analyze(
    disk: &mut crate::analysis::DiskSources,
    graphs: &mut crate::analysis::GraphCache,
    request: AnalysisRequest,
) -> SourceCompilation {
    let AnalysisTask::Query { query, cursor } = &request.task else {
        return analyze_tokens(disk, graphs, request);
    };
    let cursor = *cursor;
    let response = (|| -> Result<Option<SourceReply>> {
        request.sources.cancellation.ensure_active()?;
        let response = if matches!(query, crate::protocol::SourceQuery::Completion(_)) {
            source_completion(disk, graphs, &request, query, cursor)?
        } else {
            let include_dependents = matches!(
                query,
                crate::protocol::SourceQuery::References(_)
                    | crate::protocol::SourceQuery::Rename(_)
            );
            let names = source_graph(disk, graphs, &request, include_dependents)?;
            if crate::analysis::unproven(&names, &request.source, cursor, query).is_empty() {
                crate::analysis::respond(crate::analysis::NameGraphReply {
                    graph: &names,
                    source: &request.source,
                    cursor,
                    query,
                    view: &request.view,
                    disk: &*disk,
                    source_root: &request.source_root,
                    client_root: &crate::uri::ClientRoot::new(&request.sources.root),
                    locations: request.package_locations.as_ref(),
                    boundary: &request.boundary,
                })?
            } else {
                // A statement the graph never resolved may still be answered by the checked world.
                None
            }
        };
        request.sources.cancellation.ensure_active()?;
        let mut response = response.or_else(|| {
            request
                .view
                .is_unnamed(request.source.id())
                .then(|| crate::query::unavailable(query))
        });
        if let (Some(SourceReply::Definition(Some(definition))), Some(directory)) =
            (&mut response, request.view.package_sources())
        {
            crate::query::package_source_definition(
                definition,
                directory,
                &crate::uri::ClientRoot::new(&request.sources.root),
                &request.sources.cancellation,
            )?;
        }
        request.sources.cancellation.ensure_active()?;
        Ok(response)
    })()
    .map_err(SourceFailure::of);
    match response {
        Ok(Some(response)) => SourceCompilation::answered(request.id, request.serial, Ok(response)),
        Ok(None) => {
            let AnalysisTask::Query { query, .. } = request.task else {
                unreachable!("only a query continues to the compiler lane")
            };
            SourceCompilation::Continued(AnalysisContinuation {
                id: request.id,
                serial: request.serial,
                sources: request.sources,
                view: request.view,
                query,
            })
        }
        Err(failure) => SourceCompilation::answered(request.id, request.serial, Err(failure)),
    }
}

fn source_graph(
    disk: &mut crate::analysis::DiskSources,
    graphs: &mut crate::analysis::GraphCache,
    request: &AnalysisRequest,
    include_dependents: bool,
) -> Result<std::sync::Arc<tola_typst_syntax::imports::NameGraph>> {
    crate::analysis::source_only_graph(crate::analysis::SourceOnlyGraph {
        root: &request.source_root,
        view: &request.view,
        source: &request.source,
        locations: request.package_locations.as_ref(),
        boundary: &request.boundary,
        include_dependents,
        disk,
        cache: Some(crate::analysis::CachedGraph {
            cache: graphs,
            revision: request.sources.source_revision,
        }),
        cancellation: &request.sources.cancellation,
    })
}

fn analyze_tokens(
    disk: &mut crate::analysis::DiskSources,
    graphs: &mut crate::analysis::GraphCache,
    request: AnalysisRequest,
) -> SourceCompilation {
    // Only a range read answers for a slice; every other kind asks for the whole document.
    let range = match &request.task {
        AnalysisTask::Tokens(TokensRead {
            kind: TokensKind::Range(range),
            ..
        }) => Some(*range),
        _ => None,
    };
    let response = (|| -> Result<lsp_types::SemanticTokens> {
        request.sources.cancellation.ensure_active()?;
        let graph = source_graph(disk, graphs, &request, false)?;
        let tokens = crate::tokens::tokens(
            &request.source,
            &graph,
            range,
            &request.sources.cancellation,
        )?;
        request.sources.cancellation.ensure_active()?;
        Ok(tokens)
    })()
    .map_err(SourceFailure::of);
    let AnalysisTask::Tokens(read) = request.task else {
        unreachable!("the analysis lane tokenizes only a tokens request")
    };
    SourceCompilation::Tokens {
        id: request.id,
        serial: request.serial,
        read,
        response,
    }
}

fn source_completion(
    disk: &mut crate::analysis::DiskSources,
    graphs: &mut crate::analysis::GraphCache,
    request: &AnalysisRequest,
    query: &crate::protocol::SourceQuery,
    cursor: usize,
) -> Result<Option<SourceReply>> {
    let source = &request.source;
    let cancellation = &request.sources.cancellation;
    let names = request
        .view
        .get(query.document().0)
        .filter(|parsed| parsed.source.id() == source.id() && parsed.source.text() == source.text())
        .map(|parsed| parsed.names())
        .unwrap_or_else(|| {
            std::sync::Arc::new(tola_typst_syntax::names::SourceNames::new(source.clone()))
        });
    cancellation.ensure_active()?;
    let import = tola_typst_syntax::syntax::import(source, cursor).is_some();
    let name = crate::query::source_name_position(&names, source, cursor);
    let needs_graph = import
        || if name {
            crate::query::source_name_graph_needed(&names, cursor, cancellation)?
        } else {
            crate::query::source_member_graph_needed(&names, source, cursor, cancellation)?
        };
    let graph = if needs_graph {
        Some(crate::analysis::source_only_graph(
            crate::analysis::SourceOnlyGraph {
                root: &request.source_root,
                view: &request.view,
                source,
                locations: request.package_locations.as_ref(),
                boundary: &request.boundary,
                include_dependents: false,
                disk,
                cache: Some(crate::analysis::CachedGraph {
                    cache: graphs,
                    revision: request.sources.source_revision,
                }),
                cancellation,
            },
        )?)
    } else {
        None
    };
    let mut candidates = if import {
        crate::query::source_import_completion(
            graph.as_deref().expect("an import reads its graph"),
            &names,
            source,
            cursor,
            cancellation,
        )?
    } else {
        None
    };
    if candidates.is_none() && name {
        candidates = crate::query::source_name_completion(
            graph.as_deref(),
            &names,
            source,
            cursor,
            cancellation,
        )?;
    }
    if candidates.is_none() {
        candidates = crate::query::source_member_completion(
            graph.as_deref(),
            &names,
            source,
            cursor,
            cancellation,
        )?;
    }
    cancellation.ensure_active()?;
    Ok(candidates.map(|candidates| {
        SourceReply::Completion(lsp_types::CompletionResponse::Array(candidates))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::SourceView;
    use lsp_server::RequestId;
    use std::path::Path;
    use tola_build::cancellation::BuildCancellation;
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
            sources: crate::compiler::SourceInputs {
                root: root.to_path_buf(),
                overrides: std::sync::Arc::default(),
                source_revision: 0,
                cancellation: BuildCancellation::new(),
            },
            source_root: root.to_path_buf(),
            view: SourceView::default(),
            source,
            package_locations: None,
            task: super::super::jobs::AnalysisTask::Query {
                cursor,
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
            },
        };
        (request, text)
    }

    #[test]
    fn dynamic_references_wait_for_compiler() {
        let directory = tempfile::tempdir().unwrap();
        let root = tola_input::normalize_existing_prefix(directory.path());
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
    fn literal_references_answer_locally() {
        let directory = tempfile::tempdir().unwrap();
        let root = tola_input::normalize_existing_prefix(directory.path());
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
