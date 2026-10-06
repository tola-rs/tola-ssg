//! The compiler worker: one lane's retained session, its jobs, and their outcomes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancelled;
use tola_build::check::SourceDiagnosticSession;
use tola_typst_syntax::names::SelectedInterfaces;

use super::check::CheckedSources;
use super::compilations::RevisionCompilations;
use super::jobs::{AnalysisContinuation, QueryRequest, SourceInputs, SourceJob, UnsavedSource};
use crate::protocol::{SourceQuery, SourceReply};
use crate::server::ServedWorkspace;

#[derive(Debug)]
pub(crate) enum SourceFailure {
    Cancelled,
    Failed(anyhow::Error),
}

impl SourceFailure {
    pub(super) fn of(error: anyhow::Error) -> Self {
        if error.chain().any(|cause| cause.is::<BuildCancelled>()) {
            Self::Cancelled
        } else {
            Self::Failed(error)
        }
    }

    /// The outcome of a job whose configuration load failed.
    ///
    /// Cancellation keeps its own outcome: the requester has already left. A broken configuration
    /// answers whatever `empty` builds instead of failing the job — the failure is already
    /// published as diagnostics on the configuration file, and the job must not fail on every
    /// edit.
    pub(super) fn resolve_load_failure<T>(
        error: anyhow::Error,
        empty: impl FnOnce() -> T,
    ) -> Result<T, SourceFailure> {
        match Self::of(error) {
            failure @ Self::Cancelled => Err(failure),
            Self::Failed(_) => Ok(empty()),
        }
    }
}

impl From<BuildCancelled> for SourceFailure {
    fn from(_: BuildCancelled) -> Self {
        Self::Cancelled
    }
}

pub(crate) enum SourceCompilation {
    Checked {
        checked_revision: u64,
        checked: Result<CheckedSources, SourceFailure>,
    },
    Answered {
        id: lsp_server::RequestId,
        serial: u64,
        response: Result<SourceReply, SourceFailure>,
    },
    /// A source-only analysis whose identity proof needs the compiler lane, with the request
    /// that lane answers.
    Continued(AnalysisContinuation),
    /// The selection index one correction asked for, which no completed check could return.
    Selected {
        revision: u64,
        root: PathBuf,
        selected: Result<Arc<SelectedInterfaces>, SourceFailure>,
    },
    /// The retained compilation was let go; nothing answers to it.
    Released,
}

pub(crate) struct SourceCompiler<F> {
    pub(super) load: F,
    pub(super) resources: BuildResources,
    pub(super) published: crate::published::PublishedIndex,
    pub(super) session: Option<(ServedWorkspace, SourceDiagnosticSession)>,
    pub(super) compilations: RevisionCompilations,
    /// The site's own Typst files as this lane last read them, shared by every request it
    /// answers.
    pub(super) disk: crate::analysis::DiskSources,
    /// The site's bibliography files as this lane last parsed them, shared by every request it
    /// answers.
    pub(super) bibliographies: crate::query::BibliographyCache,
}

impl<F> SourceCompiler<F>
where
    F: FnMut(&Path, &[UnsavedSource]) -> Result<ServedWorkspace>,
{
    pub(crate) fn new(load: F, resources: BuildResources) -> Self {
        Self {
            load,
            resources,
            published: crate::published::PublishedIndex::default(),
            session: None,
            compilations: RevisionCompilations::new(),
            disk: crate::analysis::DiskSources::default(),
            bibliographies: crate::query::BibliographyCache::default(),
        }
    }

    pub(crate) fn compile(&mut self, job: SourceJob) -> SourceCompilation {
        match job {
            SourceJob::Check(request) => SourceCompilation::Checked {
                checked_revision: request.checked_revision,
                checked: self.check(&request.sources, &request.view),
            },
            SourceJob::Query(request) => SourceCompilation::Answered {
                response: self.query_reply(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::Route(request) => SourceCompilation::Answered {
                response: self.route(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::RouteIndex(request) => SourceCompilation::Answered {
                response: self.route_index(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::Lenses(request) => SourceCompilation::Answered {
                response: self.lenses(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::Symbols(request) => SourceCompilation::Answered {
                response: self.workspace_symbols(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::Rename(request) => SourceCompilation::Answered {
                response: self.rename(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::PackageSource(request) => SourceCompilation::Answered {
                response: self.package_source(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::IncomingCalls(request) => SourceCompilation::Answered {
                response: self.incoming_calls(&request),
                id: request.id,
                serial: request.serial,
            },
            SourceJob::Selection(request) => SourceCompilation::Selected {
                revision: request.revision,
                root: request.root.clone(),
                selected: crate::analysis::site_selected_interfaces(
                    &request.root,
                    &request.view,
                    &mut self.disk,
                    &request.cancellation,
                )
                .map_err(SourceFailure::of),
            },
            SourceJob::ReleaseIdle => {
                self.compilations.release();
                SourceCompilation::Released
            }
        }
    }

    pub(super) fn prepare(
        &mut self,
        sources: &SourceInputs,
    ) -> Result<(&ServedWorkspace, &mut SourceDiagnosticSession)> {
        sources.cancellation.ensure_active()?;
        let served = (self.load)(&sources.root, &sources.overrides);
        sources.cancellation.ensure_active()?;
        let served = served?;
        if self
            .session
            .as_ref()
            .is_none_or(|(active, _)| !Arc::ptr_eq(active.configuration(), served.configuration()))
        {
            let session = SourceDiagnosticSession::with_resources(
                Arc::clone(served.configuration()),
                self.resources.clone(),
            );
            self.session = Some((served, session));
        }
        Ok(Self::prepared_session(&mut self.session))
    }

    /// The session of the configuration `prepare` resolved, without borrowing the whole compiler.
    ///
    /// Jobs call `prepare` for its side effect and then read the session through this function:
    /// the borrow `prepare` returned would keep the whole compiler borrowed while the job reads
    /// other fields as well.
    pub(super) fn prepared_session(
        session: &mut Option<(ServedWorkspace, SourceDiagnosticSession)>,
    ) -> (&ServedWorkspace, &mut SourceDiagnosticSession) {
        let (served, session) = session
            .as_mut()
            .expect("prepared configuration owns source session");
        (served, session)
    }

    pub(super) fn query_reply(
        &mut self,
        request: &QueryRequest,
    ) -> Result<SourceReply, SourceFailure> {
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        let response = (|| -> Result<SourceReply, SourceFailure> {
            match self.prepare(&request.sources) {
                Ok(_) => {}
                Err(error) => {
                    return SourceFailure::resolve_load_failure(error, || {
                        crate::query::unavailable(&request.query)
                    });
                }
            }
            let (served, session) = Self::prepared_session(&mut self.session);
            let configuration = served.configuration();
            let published = self.published.snapshot(
                &self.resources,
                configuration.package_locations(),
                matches!(
                    request.query,
                    SourceQuery::Completion(_) | SourceQuery::InlayHints(_)
                ),
            );
            let mut response = crate::query::respond(
                session,
                &mut self.compilations,
                &mut self.disk,
                &mut self.bibliographies,
                configuration,
                request.sources.source_revision,
                &request.sources.overrides,
                &request.view,
                published,
                request.package_sources.as_deref().map(PathBuf::as_path),
                &request.query,
                request.routes_as_hints,
                request.check_progress,
                &request.sources.cancellation,
                &crate::uri::ClientRoot::new(&request.sources.root),
            )
            .map_err(SourceFailure::of)?;
            if let SourceReply::Definition(Some(definition)) = &mut response
                && let Some(directory) = &request.package_sources
            {
                crate::query::package_source_definition(
                    definition,
                    directory,
                    &crate::uri::ClientRoot::new(&request.sources.root),
                    &request.sources.cancellation,
                )
                .map_err(SourceFailure::of)?;
            }
            Ok(response)
        })();
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::tests::{assert_cancelled, inputs, site_configuration};
    use crate::compiler::{CheckRequest, SourceJob};
    use crate::sources::SourceView;
    use tola_build::BuildResources;
    use tola_build::cancellation::BuildCancellation;
    use tola_build::diagnostic::{Diagnostic, DiagnosticError, Severity};

    fn package_hover(sources: SourceInputs) -> SourceJob {
        let uri = crate::uri::from_file_path(&sources.root.join("site.typ")).unwrap();
        let marked = "#import \"@tola/docu|ment:0.0.0\"";
        let cursor = marked.find('|').unwrap();
        let source = tola_typst::typst::syntax::Source::detached(marked.replace('|', ""));
        let position = crate::position::utf16_range(source.lines(), cursor..cursor)
            .unwrap()
            .start;
        SourceJob::Query(QueryRequest {
            id: 7.into(),
            serial: 11,
            sources,
            view: SourceView::default(),
            package_sources: None,
            routes_as_hints: false,
            check_progress: crate::protocol::CheckProgress::NotChecked,
            query: crate::protocol::SourceQuery::Hover(lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier::new(uri),
                position,
            }),
        })
    }

    #[test]
    fn cancellation_overrides_config_errors() {
        let root = std::env::current_dir().unwrap();
        for query in [false, true] {
            for cancel_before_load in [false, true] {
                let canceller = tola_build::cancellation::BuildCanceller::new();
                if cancel_before_load {
                    canceller.cancel();
                }
                let mut compiler = SourceCompiler::new(
                    |_, _| {
                        assert!(
                            !cancel_before_load,
                            "cancelled jobs must not load configuration"
                        );
                        canceller.cancel();
                        anyhow::bail!("configuration failed while cancellation arrived")
                    },
                    BuildResources::default(),
                );
                let sources = inputs(&root, canceller.token());
                let job = if query {
                    package_hover(sources)
                } else {
                    SourceJob::Check(CheckRequest {
                        checked_revision: 3,
                        sources,
                        view: SourceView::default(),
                    })
                };
                assert_cancelled(compiler.compile(job));
            }
        }
    }

    #[test]
    fn loader_cancellation_propagates() {
        let mut compiler =
            SourceCompiler::new(|_, _| Err(BuildCancelled.into()), BuildResources::default());
        assert_cancelled(compiler.compile(SourceJob::Check(CheckRequest {
            checked_revision: 1,
            sources: inputs(Path::new("/site"), BuildCancellation::new()),
            view: SourceView::default(),
        })));
    }

    #[test]
    fn configuration_failure_retains_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(
            directory.path().join("site.typ"),
            "#import \"@tola/document:0.0.0\": current-document\n#document(\"index.html\")[Body]",
        )
        .unwrap();
        let configuration = site_configuration(directory.path(), "");
        let diagnostics = vec![
            Diagnostic::at_path(
                tola_build::codes::config::INVALID,
                Severity::Error,
                path.to_string_lossy(),
                "invalid configuration",
            )
            .with_note("retained explanation"),
            Diagnostic::at_path(
                tola_build::codes::config::WARNING,
                Severity::Warning,
                path.to_string_lossy(),
                "configuration warning",
            ),
        ];
        let mut first = true;
        let mut compiler = SourceCompiler::new(
            |_, _| {
                if std::mem::take(&mut first) {
                    Ok(ServedWorkspace::Site(Arc::clone(&configuration)))
                } else {
                    Err(anyhow::Error::new(DiagnosticError::new(
                        "configuration rejected",
                        diagnostics.clone(),
                    ))
                    .context("reload editor configuration"))
                }
            },
            BuildResources::default(),
        );
        let SourceCompilation::Answered { response, .. } = compiler.compile(package_hover(inputs(
            directory.path(),
            BuildCancellation::new(),
        ))) else {
            panic!("expected query completion")
        };
        let SourceReply::Hover(Some(hover)) = response.unwrap() else {
            panic!("expected package hover")
        };
        assert!(
            matches!(hover.contents, lsp_types::HoverContents::Markup(contents) if contents.value.contains("@tola/document"))
        );
        let SourceCompilation::Checked { checked, .. } =
            compiler.compile(SourceJob::Check(CheckRequest {
                checked_revision: 2,
                sources: inputs(directory.path(), BuildCancellation::new()),
                view: SourceView::default(),
            }))
        else {
            panic!("expected check completion")
        };
        let checked = checked.unwrap();
        assert!(Arc::ptr_eq(
            checked.configuration.as_ref().unwrap(),
            &configuration
        ));
        assert_eq!(checked.diagnostics, diagnostics);
        let SourceCompilation::Answered { response, .. } = compiler.compile(package_hover(inputs(
            directory.path(),
            BuildCancellation::new(),
        ))) else {
            panic!("expected query completion")
        };
        assert!(matches!(
            response.expect("configurations failures do not fail queries"),
            SourceReply::Hover(None)
        ));
    }
}
