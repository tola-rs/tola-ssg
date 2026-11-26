//! Connection entrypoint and host-supplied workspace configuration.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle, ScopedJoinHandle};
use std::time::Instant;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tola_build::BuildResources;
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_build::config::SiteConfig;
use tola_diagnostic::Diagnostic;

use super::events::{Event, WAIT_INTERVAL};
use super::lanes::{self, AnalysisJob, JobOwner};
use super::reader::read_messages;
use crate::compiler::{SourceCompilation, SourceCompiler, SourceJob};
use crate::connection::Connection;

const STOPPED: &str = "the language server stopped; restart it from your editor";

/// Host-declared configuration sections beyond the core schema.
pub type HostSections =
    &'static [&'static [(tola_build::config::FieldPath, Option<&'static str>)]];

/// How a workspace root is served: a full site or standalone open documents.
#[derive(Debug)]
pub enum ServedWorkspace {
    /// Full site compiled from one root Bundle.
    Site(Arc<SiteConfig>),
    /// Standalone documents compiled individually without a site config.
    Documents(Arc<SiteConfig>),
}

impl ServedWorkspace {
    /// Configuration the workspace's sources compile through.
    pub fn configuration(&self) -> &Arc<SiteConfig> {
        match self {
            Self::Site(cfg) | Self::Documents(cfg) => cfg,
        }
    }

    /// Whether the site's own program compiles the whole site.
    pub fn compiles_site(&self) -> bool {
        matches!(self, Self::Site(_))
    }
}

/// Inputs for serving one independent language-server connection.
pub struct ServeRequest<R, W, F, D>
where
    R: Read + Send + 'static,
    W: Write,
    F: FnMut(&Path, &[(PathBuf, Arc<str>)], &BuildCancellation) -> Result<ServedWorkspace>
        + Send
        + 'static,
    D: Fn(&[Diagnostic]),
{
    /// Client input stream (read on a detached thread).
    pub reader: R,
    /// Client output stream.
    pub writer: W,
    /// Resources the connection compiles with.
    pub resources: BuildResources,
    /// Ends the connection's work when cancelled.
    pub cancellation: BuildCancellation,
    /// Loads configuration from the editor root and unsaved sources for the current job.
    pub load_configuration: F,
    /// Receives complete current diagnostics before their LSP projection.
    pub record_diagnostics: D,
    /// Explicit configuration file path, if provided.
    pub named_configuration: Option<PathBuf>,
    /// Host-declared configuration sections beyond the core schema.
    pub host_sections: HostSections,
}

/// Serve one independent Tola language-server connection.
///
/// `load_configuration` receives the editor root, exact unsaved sources, and cancellation for
/// each operation; keep the `Arc` identity stable across unchanged settings to reuse compiler
/// resources. `record_diagnostics` receives diagnostics before LSP projection and must not write
/// to the protocol output. Worker and package-index threads are cancelled and joined before return,
/// while a blocked reader thread is detached so shutdown does not stall. Source checks neither run
/// hooks nor publish site output, and lane panics are contained as internal errors.
pub fn serve<R, W, F, D>(inputs: ServeRequest<R, W, F, D>) -> Result<()>
where
    R: Read + Send + 'static,
    W: Write,
    F: FnMut(&Path, &[(PathBuf, Arc<str>)], &BuildCancellation) -> Result<ServedWorkspace>
        + Send
        + 'static,
    D: Fn(&[Diagnostic]),
{
    let ServeRequest {
        reader,
        writer,
        resources,
        cancellation,
        load_configuration,
        record_diagnostics,
        named_configuration,
        host_sections,
    } = inputs;
    let (ev_tx, ev_rx) = crossbeam_channel::bounded(0);
    let (comp_tx, comp_rx) = crossbeam_channel::bounded(1);
    let (ana_tx, ana_rx) = crossbeam_channel::bounded(1);
    let shutdown = BuildCanceller::new();
    let reader_tx = ev_tx.clone();
    let reader_cancel = shutdown.token();
    let input = thread::Builder::new()
        .name("tola-lsp-reader".to_owned())
        .spawn(move || read_messages(reader, &reader_tx, &reader_cancel))
        .context("the Tola language server could not start its input reader")?;
    thread::scope(|scope| {
        // Take worker tokens before moving `shutdown` into `Connection`.
        let comp_cancel = shutdown.token();
        let ana_cancel = shutdown.token();
        let ana_tx_ev = ev_tx.clone();
        let mut conn = Connection::new(
            writer,
            record_diagnostics,
            shutdown,
            resources.clone(),
            named_configuration,
            None,
            host_sections,
        );
        let compiler_thread = scope.spawn(move || {
            let mut compiler = SourceCompiler::new(load_configuration, resources);
            lanes::run(
                comp_rx,
                &ev_tx,
                &comp_cancel,
                JobOwner::of,
                |job| compiler.compile(job),
                Event::Compiled,
            );
        });
        let analysis_thread = scope.spawn(move || {
            let mut disk = crate::analysis::DiskSources::default();
            let mut graphs = crate::analysis::GraphCache::default();
            lanes::run(
                ana_rx,
                &ana_tx_ev,
                &ana_cancel,
                JobOwner::of_analysis,
                |job| match job {
                    AnalysisJob::Request(req) => {
                        crate::compiler::analyze(&mut disk, &mut graphs, *req)
                    }
                    AnalysisJob::ReleaseIdle => {
                        disk = Default::default();
                        graphs = Default::default();
                        SourceCompilation::Released
                    }
                },
                Event::Analyzed,
            );
        });
        let mut lanes = Lanes {
            compiler_jobs: Some(comp_tx),
            compiler_thread,
            analysis_jobs: Some(ana_tx),
            analysis_thread,
        };
        let outcome = drive_connection(&mut conn, &ev_rx, &mut lanes, &input, &cancellation);
        end_connection(conn, lanes, input, outcome)
    })
}

struct Lanes<'scope> {
    compiler_jobs: Option<Sender<SourceJob>>,
    compiler_thread: ScopedJoinHandle<'scope, ()>,
    analysis_jobs: Option<Sender<AnalysisJob>>,
    analysis_thread: ScopedJoinHandle<'scope, ()>,
}

impl Lanes<'_> {
    fn dispatch_compilation(&mut self, job: SourceJob) -> Result<()> {
        self.compiler_jobs
            .as_ref()
            .expect("a serving connection owns the compiler lane")
            .send(job)
            .context("Tola compiler lane has stopped")
    }

    fn dispatch_analysis(&mut self, job: AnalysisJob) -> Result<()> {
        self.analysis_jobs
            .as_ref()
            .expect("a serving connection owns the analysis lane")
            .send(job)
            .context("Tola analysis lane has stopped")
    }

    fn close(&mut self) {
        self.compiler_jobs = None;
        self.analysis_jobs = None;
    }

    fn any_finished(&self) -> bool {
        self.compiler_thread.is_finished() || self.analysis_thread.is_finished()
    }

    fn join(self) -> Result<()> {
        let comp = self.compiler_thread.join().map_err(|_| stopped_error());
        let ana = self.analysis_thread.join().map_err(|_| stopped_error());
        comp.and(ana)
    }
}

fn drive_connection<W, D>(
    conn: &mut Connection<W, D>,
    ev_rx: &Receiver<Event>,
    lanes: &mut Lanes<'_>,
    input: &JoinHandle<()>,
    cancel: &BuildCancellation,
) -> Result<()>
where
    W: Write,
    D: Fn(&[Diagnostic]),
{
    let mut compiling = false;
    let mut analyzing = false;
    loop {
        cancel.ensure_active()?;
        if conn.is_shutdown() {
            lanes.close();
        } else {
            let now = Instant::now();
            if conn.run_due_work(now, compiling || analyzing)? {
                lanes.dispatch_compilation(SourceJob::ReleaseIdle)?;
                lanes.dispatch_analysis(AnalysisJob::ReleaseIdle)?;
                compiling = true;
                analyzing = true;
            } else {
                // Only dispatch to an idle lane so bounded channels stay backpressured.
                if !compiling && let Some(job) = conn.next_job(now) {
                    lanes.dispatch_compilation(job)?;
                    compiling = true;
                }
                if !analyzing && let Some(req) = conn.next_analysis() {
                    lanes.dispatch_analysis(AnalysisJob::Request(Box::new(req)))?;
                    analyzing = true;
                }
            }
        }
        let ev = match ev_rx.recv_timeout(WAIT_INTERVAL) {
            Ok(ev) => ev,
            Err(RecvTimeoutError::Timeout) => {
                if input.is_finished() || (lanes.any_finished() && !conn.is_shutdown()) {
                    return Err(stopped_error());
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        match ev {
            Event::Message(msg) => {
                if conn.receive(msg)?.is_break() {
                    return Ok(());
                }
            }
            Event::Compiled(done) => {
                compiling = false;
                conn.completed(done)?;
            }
            Event::Analyzed(done) => {
                analyzing = false;
                conn.completed(done)?;
            }
            Event::InputClosed => return Ok(()),
            Event::InputFailed(err) => return Err(err.into()),
        }
    }
}

fn end_connection<W, D>(
    conn: Connection<W, D>,
    mut lanes: Lanes<'_>,
    input: JoinHandle<()>,
    outcome: Result<()>,
) -> Result<()> {
    conn.cancel();
    lanes.close();
    // Joining a reader blocked on stdin would hang shutdown.
    let reader_joined = if input.is_finished() {
        input.join().map_err(|_| stopped_error())
    } else {
        Ok(())
    };
    let lanes_joined = lanes.join();
    match outcome {
        Err(err) if client_disconnected(&err) => reader_joined.and(lanes_joined),
        outcome => outcome.and(reader_joined).and(lanes_joined),
    }
}

fn stopped_error() -> anyhow::Error {
    anyhow::anyhow!(STOPPED)
}

fn client_disconnected(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
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
        let pkg = client.package_source("tola-package:/tola/document/0.0.0/lib.typ");

        site.write("tola.toml", "[site\n");
        client.changed_on_disk("tola.toml");
        let diags = client.diagnostics_matching("tola.toml", |published| !published.is_empty());
        assert!(!diags.is_empty());
        assert_eq!(
            client.package_source("tola-package:/tola/document/0.0.0/lib.typ"),
            pkg
        );

        client.shutdown();
    }

    #[test]
    fn narrowing_actions_observed_sources() {
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
        let actions = client.code_actions("site.typ", &marked, &[], &["refactor"]);
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
    fn unknown_variable_quickfix() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        let text = "#let missing = absent-name\n\n#missing\n";
        client.open("content/document.typ", text);

        let diags = client.diagnostics_matching("content/document.typ", |diags| {
            diags.iter().any(|diag| diag.data.is_some())
        });
        let diag = diags
            .iter()
            .find(|diag| diag.data.is_some())
            .expect("the unbound name is reported");
        assert_eq!(
            diag.data
                .as_ref()
                .and_then(|cause| cause.get("name"))
                .and_then(serde_json::Value::as_str),
            Some("absent-name")
        );

        let actions = client.code_actions(
            "content/document.typ",
            &format!("{text}|"),
            std::slice::from_ref(diag),
            &["quickfix"],
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
                .any(|decl| decl.name == "absent-name")
        );
        client.set_text("content/document.typ", names.source().text());
        assert!(client.diagnostics("content/document.typ").is_empty());

        client.shutdown();
    }

    fn action_edits(action: &serde_json::Value, uri: &str) -> Vec<TextEdit> {
        let mut edit =
            lsp_types::WorkspaceEdit::deserialize(&action["edit"]).expect("an applicable edit");
        let uri: Uri = uri.parse().expect("the target document URI");
        let docs = match edit.document_changes {
            Some(lsp_types::DocumentChanges::Edits(docs)) => docs,
            Some(lsp_types::DocumentChanges::Operations(ops)) => ops
                .into_iter()
                .filter_map(|op| match op {
                    lsp_types::DocumentChangeOperation::Edit(doc) => Some(doc),
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
        docs.into_iter()
            .filter(|doc| doc.text_document.uri == uri)
            .flat_map(|doc| {
                doc.edits.into_iter().map(|edit| match edit {
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
    fn rejected_request_recovers() {
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
    fn client_disconnect_ends_session() {
        let site = SiteDirectory::new();
        let client = EditorSession::start(&site);
        client.abort();
    }
}
