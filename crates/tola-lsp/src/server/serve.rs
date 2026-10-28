//! Serving one connection: the public entry point a host calls, the host-declared configuration
//! sections it passes, and what its workspace root is served as.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use crossbeam_channel::RecvTimeoutError;
use tola_build::BuildResources;
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_build::config::ResolvedSiteConfig;
use tola_build::diagnostic::Diagnostic;

use super::events::{Event, WAIT_INTERVAL};
use super::lanes::{self, JobOwner};
use super::reader::read_messages;
use crate::compiler::SourceCompiler;
use crate::connection::Connection;

const STOPPED: &str = "the language server stopped; restart it from your editor";

/// The configuration sections a host declares beyond the core schema, each as the field list its
/// own `Config` derive generated.
///
/// A `tola.toml` key is answered from the schema that declares it, so a host hands over its
/// sections' declarations rather than the server hard-coding a second list.
pub type HostSections =
    &'static [&'static [(tola_build::config::FieldPath, Option<&'static str>)]];

/// What one workspace root is served as.
///
/// A workspace that holds no site configuration is served as the documents it holds: each open
/// document is checked on its own, and no answer claims what a site's program establishes.
#[derive(Debug)]
pub enum ServedWorkspace {
    /// The workspace's site, which one root Bundle compiles.
    Site(Arc<ResolvedSiteConfig>),
    /// The workspace's documents, each compiled in the world its own imports resolve.
    Documents(Arc<ResolvedSiteConfig>),
}

impl ServedWorkspace {
    /// The configuration the workspace's sources compile through.
    pub fn configuration(&self) -> &Arc<ResolvedSiteConfig> {
        match self {
            Self::Site(configuration) | Self::Documents(configuration) => configuration,
        }
    }

    /// Whether the site's own program compiles the whole site.
    pub fn compiles_site(&self) -> bool {
        matches!(self, Self::Site(_))
    }
}

/// Serve one independent Tola language-server connection.
///
/// `load_configuration` receives the editor root and exact unsaved sources for
/// each operation, and reports the workspace as the site it holds, or as the
/// documents it holds when no site configuration exists. Keep the configuration's
/// `Arc` identity stable while build settings are unchanged to reuse compiler
/// resources. Load errors retain structured diagnostics. Host-only settings stay
/// in the callback; the host's own key declarations travel
/// in `host_sections`, so the server answers `tola.toml` keys it did not declare itself.
/// `resources` selects network access, fonts and reusable file inputs for both
/// source compilation and published-package completion.
///
/// `record_diagnostics` receives complete current diagnostics before their LSP
/// projection. It must not write to the protocol output.
///
/// Compiler, source-analysis and package-index work is cancelled and joined before return.
/// A reader still blocked on client input is detached, so an open input stream
/// cannot delay shutdown.
/// Source checks neither run hooks nor publish site output.
///
/// A job whose own work panics is answered as an internal failure and the lane keeps running. A
/// client that stops reading blocks the connection inside its next write: reads run on the detached
/// reader thread, and the host's token is observed between messages, so no cancellation reaches a
/// write already in flight.
#[expect(
    clippy::too_many_arguments,
    reason = "transport, compilation and host callbacks are independent connection inputs"
)]
pub fn serve<R, W, F, D>(
    reader: R,
    writer: W,
    resources: BuildResources,
    cancellation: BuildCancellation,
    load_configuration: F,
    record_diagnostics: D,
    named_configuration: Option<PathBuf>,
    host_sections: HostSections,
) -> Result<()>
where
    R: Read + Send + 'static,
    W: Write,
    F: FnMut(&Path, &[(PathBuf, Arc<str>)]) -> Result<ServedWorkspace> + Send + 'static,
    D: Fn(&[Diagnostic]),
{
    let (events, incoming) = crossbeam_channel::bounded(0);
    let (jobs, pending) = crossbeam_channel::bounded(1);
    let (analyses, pending_analyses) = crossbeam_channel::bounded(1);
    let shutdown = BuildCanceller::new();
    let reader_events = events.clone();
    let reader_cancel = shutdown.token();
    let input = thread::Builder::new()
        .name("tola-lsp-reader".to_owned())
        .spawn(move || read_messages(reader, &reader_events, &reader_cancel))
        .context("the Tola language server could not start its input reader")?;
    thread::scope(|scope| {
        let mut jobs = Some(jobs);
        let mut analyses = Some(analyses);
        let analysis_cancel = shutdown.token();
        let analysis_events = events.clone();
        let compiler_cancel = shutdown.token();
        let mut connection = Connection::new(
            writer,
            record_diagnostics,
            shutdown,
            resources.clone(),
            named_configuration,
            // The origin this site is served at comes from the client's launch configuration and
            // the development server's own state, not from this process.
            None,
            host_sections,
        );
        let compiler = scope.spawn(move || {
            let mut compiler = SourceCompiler::new(load_configuration, resources);
            lanes::run(
                pending,
                &events,
                &compiler_cancel,
                JobOwner::of,
                |job| compiler.compile(job),
                Event::Compiled,
            );
        });
        let analyzer = scope.spawn(move || {
            let mut disk = crate::analysis::DiskSources::default();
            let mut graphs = crate::analysis::GraphCache::default();
            lanes::run(
                pending_analyses,
                &analysis_events,
                &analysis_cancel,
                JobOwner::of_analysis,
                |request| crate::compiler::analyze(&mut disk, &mut graphs, request),
                Event::Analyzed,
            );
        });
        let outcome = (|| -> Result<()> {
            let mut compiling = false;
            let mut analyzing = false;
            loop {
                cancellation.ensure_active()?;
                if connection.is_shutdown() {
                    drop(jobs.take());
                    drop(analyses.take());
                } else {
                    // A report that has run long enough to show, and a site nothing is using, are
                    // owed between jobs: a job in flight is what both are waiting on.
                    connection.run_due_work(Instant::now())?;
                    if !compiling && let Some(job) = connection.next_job(Instant::now()) {
                        // A lane receives another job only after its preceding reply arrives.
                        jobs.as_ref()
                            .expect("active connection owns compiler input")
                            .send(job)
                            .context("Tola compiler lane has stopped")?;
                        compiling = true;
                    }
                    if !analyzing && let Some(request) = connection.next_analysis() {
                        analyses
                            .as_ref()
                            .expect("active connection owns analysis input")
                            .send(request)
                            .context("Tola analysis lane has stopped")?;
                        analyzing = true;
                    }
                }
                let event = match incoming.recv_timeout(WAIT_INTERVAL) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => {
                        if input.is_finished() {
                            bail!("{STOPPED}");
                        }
                        if (compiler.is_finished() || analyzer.is_finished())
                            && !connection.is_shutdown()
                        {
                            bail!("{STOPPED}");
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                };
                match event {
                    Event::Message(message) => {
                        if connection.receive(message)?.is_break() {
                            return Ok(());
                        }
                    }
                    Event::Compiled(completed) => {
                        compiling = false;
                        connection.completed(completed)?;
                    }
                    Event::Analyzed(completed) => {
                        analyzing = false;
                        connection.completed(completed)?;
                    }
                    Event::InputClosed => return Ok(()),
                    Event::InputFailed(error) => return Err(error),
                }
            }
        })();
        connection.cancel();
        drop(jobs.take());
        drop(analyses.take());
        let reader_joined = if input.is_finished() {
            input.join().map_err(|_| anyhow::anyhow!(STOPPED))
        } else {
            Ok(())
        };
        let workers_joined = compiler
            .join()
            .map_err(|_| anyhow::anyhow!(STOPPED))
            .and(analyzer.join().map_err(|_| anyhow::anyhow!(STOPPED)));
        match outcome {
            Err(error) if disconnected(&error) => reader_joined.and(workers_joined),
            outcome => outcome.and(reader_joined).and(workers_joined),
        }
    })
}

fn disconnected(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use lsp_server::ErrorCode;
    use lsp_types::request::Request as LspRequest;
    use lsp_types::{TextEdit, Uri};
    use serde::Deserialize;
    use tola_typst::typst::syntax::Source;

    use crate::position;
    use crate::server::tests::{EditorSession, SiteDirectory, site_with_document, uri_string};
    #[test]
    fn package_sources_outlive_broken_config() {
        let site = SiteDirectory::new();
        let mut client = EditorSession::start(&site);
        let package = client.package_source("tola-package:/tola/document/0.0.0/lib.typ");

        site.write("tola.toml", "[site\n");
        client.changed_on_disk("tola.toml");
        let diagnostics =
            client.diagnostics_matching("tola.toml", |published| !published.is_empty());
        assert!(
            !diagnostics.is_empty(),
            "the broken configuration is reported"
        );
        assert_eq!(
            client.package_source("tola-package:/tola/document/0.0.0/lib.typ"),
            package
        );

        client.shutdown();
    }
    /// A metadata chain offers one narrowing action per source the checked world observed.
    #[test]
    fn narrowing_actions_follow_observed_sources() {
        const PROGRAM: &str = r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": join-route, output-of, slugify
#import "@tola/schema:0.0.0": describe, optional, schema
#let page-schema = schema((
  draft: describe(optional(bool, default: false), "keeps the page out of the published site"),
))
#let declared = parse-sources(all-sources(), page-schema)
#let kept = declared.filter(source => not source.meta.draft)
#for source in kept {
  document(output-of(join-route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))))[#include source.file]
}
"#;
        let site = SiteDirectory::new();
        site.write("site.typ", PROGRAM);
        site.write("content/a.typ", "Body A.\n");
        site.write("content/b.typ", "Body B.\n");
        let mut client = EditorSession::start(&site);

        let marked = PROGRAM.replace("source.meta.draft", "source.meta.dr|aft");
        let actions = client.code_actions_of_kind("site.typ", &marked, &["refactor"]);
        let titles = actions
            .iter()
            .filter_map(|action| action["title"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            ["restrict to `content/a.typ`", "restrict to `content/b.typ`"],
            "{actions:?}"
        );
        let edits = action_edits(&actions[0], &uri_string(site.root(), "site.typ"));
        assert_eq!(
            edits.first().map(|edit| edit.new_text.as_str()),
            Some("source.path == \"a.typ\" and (not source.meta.draft)"),
            "{actions:?}"
        );

        client.shutdown();
    }
    #[test]
    fn unknown_variable_offers_definition() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        let text = "#let missing = absent-name\n\n#missing\n";
        client.open("content/document.typ", text);

        let diagnostics = client.diagnostics_matching("content/document.typ", |diagnostics| {
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.data.is_some())
        });
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.data.is_some())
            .expect("the unbound name is reported");
        assert_eq!(
            diagnostic
                .data
                .as_ref()
                .and_then(|cause| cause.get("name"))
                .and_then(serde_json::Value::as_str),
            Some("absent-name")
        );

        let actions = client.code_actions_for_diagnostics(
            "content/document.typ",
            &format!("{text}|"),
            std::slice::from_ref(diagnostic),
        );
        assert_eq!(actions.len(), 1, "{actions:?}");
        let edits = action_edits(
            &actions[0],
            &uri_string(site.root(), "content/document.typ"),
        );
        let names = tola_typst_syntax::names::SourceNames::new(Source::detached(applied_source(
            text, &edits,
        )));
        assert!(
            names
                .declarations()
                .iter()
                .any(|declaration| declaration.name == "absent-name")
        );
        client.set_text("content/document.typ", names.source().text());
        assert!(client.diagnostics("content/document.typ").is_empty());

        client.shutdown();
    }

    fn action_edits(action: &serde_json::Value, uri: &str) -> Vec<TextEdit> {
        let mut edit =
            lsp_types::WorkspaceEdit::deserialize(&action["edit"]).expect("an applicable edit");
        let uri: Uri = uri.parse().expect("the target document URI");
        let documents = match edit.document_changes {
            Some(lsp_types::DocumentChanges::Edits(documents)) => documents,
            Some(lsp_types::DocumentChanges::Operations(operations)) => operations
                .into_iter()
                .filter_map(|operation| match operation {
                    lsp_types::DocumentChangeOperation::Edit(document) => Some(document),
                    lsp_types::DocumentChangeOperation::Op(_) => None,
                })
                .collect(),
            None => {
                return edit
                    .changes
                    .as_mut()
                    .and_then(|changes| changes.remove(&uri))
                    .unwrap_or_default();
            }
        };
        documents
            .into_iter()
            .filter(|document| document.text_document.uri == uri)
            .flat_map(|document| {
                document.edits.into_iter().map(|edit| match edit {
                    lsp_types::OneOf::Left(edit) => edit,
                    lsp_types::OneOf::Right(edit) => edit.text_edit,
                })
            })
            .collect()
    }

    fn applied_source(text: &str, edits: &[TextEdit]) -> String {
        let lines = tola_typst::typst::syntax::Lines::new(text);
        let mut replacements = edits
            .iter()
            .map(|edit| {
                let start = position::byte_offset(&lines, edit.range.start).expect("an edit start");
                let end = position::byte_offset(&lines, edit.range.end).expect("an edit end");
                (start..end, edit.new_text.as_str())
            })
            .collect::<Vec<_>>();
        replacements.sort_by_key(|(range, _)| range.start);
        for pair in replacements.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start);
        }
        let mut corrected = text.to_owned();
        for (range, replacement) in replacements.into_iter().rev() {
            corrected.replace_range(range, replacement);
        }
        corrected
    }
    #[test]
    fn rejected_request_keeps_session_usable() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        client.open("content/document.typ", "Document body.\n");

        let (code, _) = client.error_of(
            lsp_types::request::Completion::METHOD,
            serde_json::json!({
                "textDocument": { "uri": site.uri("content/document.typ") },
                "position": { "line": "wrong", "character": 0 },
            }),
        );
        assert_eq!(code, ErrorCode::InvalidParams as i32);
        let (code, _) = client.error_of("tola/unknown", serde_json::json!({}));
        assert_eq!(code, ErrorCode::MethodNotFound as i32);

        let completions = client.complete_marked("content/document.typ", "#let answer = 1\n#ans|");
        assert!(completions.iter().any(|item| item.label == "answer"));

        client.shutdown();
    }
    #[test]
    fn losing_the_client_ends_the_session() {
        let site = SiteDirectory::new();
        let client = EditorSession::start(&site);
        client.abort();
    }
}
