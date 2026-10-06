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
    use super::*;
    use lsp_server::ErrorCode;
    use lsp_types::request::Request as LspRequest;
    use lsp_types::{TextEdit, Uri};
    use serde::Deserialize;
    use tola_build::config::loading::{BuildOverrides, CONFIG_FILE_NAME, resolve_site_config};
    use tola_typst::typst::syntax::Source;

    use crate::position;
    use crate::server::tests::{
        EditorSession, SiteDirectory, announcing_client, diagnostic_messages, site_with_document,
        uri_string,
    };

    fn spaces(tab_size: u32) -> lsp_types::FormattingOptions {
        lsp_types::FormattingOptions {
            tab_size,
            insert_spaces: true,
            ..lsp_types::FormattingOptions::default()
        }
    }

    /// A site whose saved `content/document.typ` holds `saved`, with `opened` in the editor.
    fn unsaved_document(saved: &str, opened: &str) -> (SiteDirectory, EditorSession) {
        let site = SiteDirectory::new();
        site.write("content/document.typ", saved);
        let mut client = EditorSession::start(&site);
        client.open("content/document.typ", opened);
        (site, client)
    }

    fn default_configuration(root: &Path) -> Result<Arc<ResolvedSiteConfig>> {
        Ok(Arc::new(
            resolve_site_config(
                &root.join(CONFIG_FILE_NAME),
                "",
                tola_typst::PackageLocations::from_absolute_roots(None, None)?,
                &BuildOverrides::default(),
            )?
            .into_config(),
        ))
    }

    /// A session over a workspace that holds no site configuration, served as its documents.
    fn documents_session(site: &SiteDirectory) -> EditorSession {
        EditorSession::start_with_loader(site, |root, _| {
            Ok(ServedWorkspace::Documents(default_configuration(root)?))
        })
    }

    /// A workspace that holds no site configuration is served as the documents it holds: the file
    /// the author opens answers from what it alone establishes.
    #[test]
    fn documents_answer_without_site_configuration() {
        let site = SiteDirectory::documents();
        let mut client = documents_session(&site);

        let hover = client
            .hover_marked(
                "content/document.typ",
                "#let brand = rgb(\"#777\")\n#bra|nd\n",
            )
            .expect("a hover about a document of a workspace that holds no site configuration");
        let lsp_types::HoverContents::Markup(markup) = hover.contents else {
            panic!("plain markdown, got {:?}", hover.contents);
        };
        assert!(
            markup.value.contains("let brand = color;"),
            "{}",
            markup.value
        );

        client.shutdown();
    }

    /// A document of a workspace that holds no site configuration reports its own failure: no
    /// site's program exists to report instead.
    #[test]
    fn documents_report_their_own_failure() {
        let site = SiteDirectory::documents();
        let mut client = documents_session(&site);
        let text = "#let nothing = undefined-thing\n";
        site.write("content/document.typ", text);

        client.open("content/document.typ", text);
        let diagnostics = client.diagnostics("content/document.typ");

        let messages = diagnostic_messages(&diagnostics);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("undefined-thing")),
            "the document's own failure is reported: {messages:?}"
        );
        assert!(
            messages
                .iter()
                .all(|message| !message.contains("site.typ") && !message.contains("tola.toml")),
            "no site program reports instead: {messages:?}"
        );

        client.shutdown();
    }

    #[test]
    fn unsaved_buffer_is_what_the_editor_sees() {
        let site = SiteDirectory::new();
        site.write("content/document.typ", "Saved document body.\n");
        let mut client = EditorSession::start(&site);

        client.open("content/document.typ", "#undefined_from_unsaved_buffer\n");
        let diagnostics = client.diagnostics("content/document.typ");
        assert!(
            diagnostic_messages(&diagnostics)
                .iter()
                .any(|message| message.contains("undefined_from_unsaved_buffer")),
            "{diagnostics:?}"
        );

        client.set_text("content/document.typ", "Unsaved body.\n");
        assert!(client.diagnostics("content/document.typ").is_empty());
        assert_eq!(site.read("content/document.typ"), "Saved document body.\n");

        client.shutdown();
    }

    #[test]
    fn newest_edit_decides_diagnostics() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);

        client.open("content/document.typ", "#first_problem\n");
        assert!(
            diagnostic_messages(&client.diagnostics("content/document.typ"))
                .iter()
                .any(|message| message.contains("first_problem"))
        );

        // Both edits arrive before the first check finishes: the editor keeps typing.
        client.set_text("content/document.typ", "Recovered body.\n");
        client.set_text("content/document.typ", "#second_problem\n");
        let diagnostics = client.diagnostics("content/document.typ");
        assert!(
            diagnostic_messages(&diagnostics)
                .iter()
                .any(|message| message.contains("second_problem")),
            "{diagnostics:?}"
        );
        assert!(
            !diagnostic_messages(&diagnostics)
                .iter()
                .any(|message| message.contains("first_problem")),
            "{diagnostics:?}"
        );

        client.shutdown();
    }

    #[test]
    fn new_entry_is_checked_after_config_change() {
        let site = SiteDirectory::empty();
        site.write("site.typ", "#unknown_in_entry\n");
        let mut client = EditorSession::start(&site);
        let diagnostics =
            client.diagnostics_matching("site.typ", |published| !published.is_empty());
        assert!(
            diagnostic_messages(&diagnostics)
                .iter()
                .any(|message| message.contains("unknown_in_entry")),
            "{diagnostics:?}"
        );

        site.write("other.typ", "#unknown_in_selected_entry\n");
        site.write("tola.toml", "[build]\nentry = \"other.typ\"\n");
        client.changed_on_disk("tola.toml");
        let diagnostics =
            client.diagnostics_matching("other.typ", |published| !published.is_empty());
        assert!(
            diagnostic_messages(&diagnostics)
                .iter()
                .any(|message| message.contains("unknown_in_selected_entry")),
            "{diagnostics:?}"
        );

        client.shutdown();
    }

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

    #[test]
    fn closing_document_clears_diagnostics() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);

        client.open("content/document.typ", "#undefined_in_open_document\n");
        assert!(!client.diagnostics("content/document.typ").is_empty());

        client.close("content/document.typ");
        assert!(
            client
                .diagnostics_matching("content/document.typ", |published| published.is_empty())
                .is_empty()
        );

        client.shutdown();
    }

    #[test]
    fn configuration_keys_complete() {
        let site = SiteDirectory::new();
        let mut client = EditorSession::start(&site);

        let items = client.complete_marked("tola.toml", "[build]\n|");
        assert!(
            items.iter().any(|item| item.label == "entry"),
            "`entry` missing from {items:?}"
        );
        assert!(
            !items.iter().any(|item| item.label == "title"),
            "`title` completes outside `[site]`: {items:?}"
        );

        client.shutdown();
    }

    #[test]
    fn unasked_action_kinds_answer_nothing() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        let marked = "#let url = slu|gify(\"Hello\")\n";

        assert!(
            !client
                .code_actions_of_kind("content/document.typ", marked, &["quickfix"])
                .is_empty(),
            "a quick fix is what this server offers"
        );
        assert!(
            client
                .code_actions_of_kind("content/document.typ", marked, &["refactor"])
                .is_empty(),
            "a refactor is not"
        );

        client.shutdown();
    }

    /// A metadata chain offers one narrowing action per source the checked world observed.
    #[test]
    fn narrowing_actions_follow_observed_sources() {
        const PROGRAM: &str = r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": route, route-to-output, slugify
#import "@tola/schema:0.0.0": describe, optional, schema
#let page-schema = schema((
  draft: describe(optional(bool, default: false), "keeps the page out of the published site"),
))
#let declared = parse-sources(all-sources(), page-schema)
#let kept = declared.filter(source => not source.meta.draft)
#for source in kept {
  document(route-to-output(route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))))[#include source.file]
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
    fn configuration_field_diagnostic_offers_removal() {
        let site = SiteDirectory::new();
        site.write(
            "tola.toml",
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n\n[build.minfy]\nunknown = 1\n",
        );
        let mut client = EditorSession::start(&site);
        let text = std::fs::read_to_string(site.path("tola.toml")).expect("the configuration file");
        client.open("tola.toml", &text);

        let diagnostics =
            client.diagnostics_matching("tola.toml", |diagnostics| !diagnostics.is_empty());
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.data.is_some())
            .expect("the unknown key is reported");
        assert_eq!(
            diagnostic.data,
            Some(serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [{
                    "name": "build.minfy",
                    "line": 5,
                    "written": {
                        "start": { "line": 4, "character": 7 },
                        "end": { "line": 4, "character": 12 },
                    },
                }],
            }))
        );

        let actions = client.code_actions_for_diagnostics(
            "tola.toml",
            &format!("{text}|"),
            std::slice::from_ref(diagnostic),
        );
        let removal = actions
            .iter()
            .find(|action| {
                action_edits(action, &uri_string(site.root(), "tola.toml"))
                    .iter()
                    .any(|edit| edit.new_text.is_empty())
            })
            .unwrap_or_else(|| panic!("{actions:?}"));
        let edits = action_edits(removal, &uri_string(site.root(), "tola.toml"));
        let corrected = applied_source(&text, &edits);
        let configuration = toml_edit::Document::parse(&corrected).unwrap();
        assert!(
            configuration["build"]
                .as_table()
                .unwrap()
                .get("minfy")
                .is_none()
        );
        assert_eq!(configuration["build"]["entry"].as_str(), Some("site.typ"));

        client.shutdown();
    }

    #[test]
    fn configuration_fix_all_answers_every_unknown_key() {
        let site = SiteDirectory::new();
        site.write(
            "tola.toml",
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n\n[build.minfy]\nunknown = 1\n\n[build.cach]\nunknown = 2\n",
        );
        let mut client = EditorSession::start(&site);
        let text = std::fs::read_to_string(site.path("tola.toml")).expect("the configuration file");
        client.open("tola.toml", &text);

        let diagnostics = client.diagnostics_matching("tola.toml", |diagnostics| {
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.data.is_some())
        });
        let echoed = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.data.is_some())
            .cloned()
            .collect::<Vec<_>>();

        let marked = format!("{text}|");
        let actions = client.code_actions("tola.toml", &marked, &echoed, &["source.fixAll"]);
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(actions[0]["kind"], serde_json::json!("source.fixAll"));
        assert_eq!(
            actions[0]["title"],
            serde_json::json!("fix every diagnostic in this file")
        );
        let edits = action_edits(&actions[0], &uri_string(site.root(), "tola.toml"));
        let corrected = applied_source(&text, &edits);
        assert!(!corrected.contains("minfy"), "{corrected}");
        assert!(!corrected.contains("cach"), "{corrected}");

        let unfiltered = client.code_actions("tola.toml", &marked, &echoed, &[]);
        assert!(
            unfiltered
                .iter()
                .all(|action| action["kind"] != serde_json::json!("source.fixAll")),
            "{unfiltered:?}"
        );

        client.shutdown();
    }

    #[test]
    fn named_configuration_answers_its_own_document() {
        // The configuration an author names is not always the root's `tola.toml`, and this site
        // does not compile: the named document still answers the key it misspells.
        let site = SiteDirectory::empty();
        site.write("tola.toml", "invalid TOML");
        site.write(
            "selected.toml",
            "[site]\ntitle = \"Editor contract\"\ndescription = \"Typo\"\n",
        );
        let mut client = EditorSession::start_named(&site, site.path("selected.toml"));
        let marked = "[site]\ntitle = \"Editor contract\"\ndescript|io = \"Typo\"\n";
        client.open("selected.toml", &marked.replace('|', ""));

        // The request has no diagnostics, so only the document itself can answer it.
        let actions = client.code_actions_of_kind("selected.toml", marked, &["quickfix"]);
        assert!(
            actions
                .iter()
                .any(|action| action["title"] == "replace the key with `description`"),
            "{actions:?}"
        );

        client.shutdown();
    }

    #[test]
    fn misspelled_configuration_field_offers_the_declared_key() {
        let site = SiteDirectory::new();
        let text = "[site]\ntitle = \"Editor contract\"\ndescriptio = \"Typo\"\n";
        site.write("tola.toml", text);
        let mut client = EditorSession::start(&site);
        client.open("tola.toml", text);

        let diagnostics =
            client.diagnostics_matching("tola.toml", |diagnostics| !diagnostics.is_empty());
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.data.is_some())
            .expect("the unknown key is reported");

        // The cursor stands away from the key, so the fix comes from the diagnostic's payload
        // rather than from the position the client asked about.
        let actions = client.code_actions_for_diagnostics(
            "tola.toml",
            &format!("{text}|"),
            std::slice::from_ref(diagnostic),
        );
        let edits = action_edits(&actions[0], &uri_string(site.root(), "tola.toml"));
        let corrected = applied_source(text, &edits);
        let configuration = toml_edit::Document::parse(&corrected).unwrap();
        assert_eq!(configuration["site"]["description"].as_str(), Some("Typo"));
        assert!(
            configuration["site"]
                .as_table()
                .unwrap()
                .get("descriptio")
                .is_none()
        );

        client.shutdown();
    }

    #[test]
    fn unknown_variable_diagnostic_offers_definition() {
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

    /// The source `unread_import_session` opens: two imports of a helper file, neither read.
    const UNREAD_IMPORT_SOURCE: &str =
        "#import \"../helpers.typ\": first\n#import \"../helpers.typ\": second\nBody.\n";

    /// A site whose open document imports two helpers it never reads, with the reports the
    /// document's own check published about them.
    fn unread_import_session() -> (SiteDirectory, EditorSession, Vec<lsp_types::Diagnostic>) {
        let site = site_with_document();
        site.write("helpers.typ", "#let first = 1\n#let second = 2\n");
        let mut client = EditorSession::start(&site);
        client.open("content/document.typ", UNREAD_IMPORT_SOURCE);
        let reports = client
            .diagnostics_matching("content/document.typ", |diagnostics| {
                diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.data.is_some())
                    .count()
                    >= 2
            })
            .into_iter()
            .filter(|diagnostic| diagnostic.data.is_some())
            .collect();
        (site, client, reports)
    }

    #[test]
    fn fix_all_removes_each_unused_import() {
        let features = crate::capabilities::ClientFeatures::new(&announcing_client());
        let lsp_types::CodeActionProviderCapability::Options(options) =
            crate::protocol::initialize_result(&features)
                .capabilities
                .code_action_provider
                .expect("a code action provider")
        else {
            panic!("the provider offers options");
        };
        let kinds = options.code_action_kinds.expect("advertised kinds");
        assert!(
            kinds.contains(&lsp_types::CodeActionKind::SOURCE_FIX_ALL),
            "{kinds:?}"
        );

        let (site, mut client, echoed) = unread_import_session();
        assert_eq!(echoed.len(), 2, "{echoed:?}");
        assert!(
            echoed.iter().all(|diagnostic| {
                diagnostic
                    .data
                    .as_ref()
                    .is_some_and(|cause| cause["kind"] == "unread-import")
            }),
            "{echoed:?}"
        );

        let marked = format!("{UNREAD_IMPORT_SOURCE}|");
        let actions =
            client.code_actions("content/document.typ", &marked, &echoed, &["source.fixAll"]);
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(actions[0]["kind"], serde_json::json!("source.fixAll"));
        assert_eq!(
            actions[0]["title"],
            serde_json::json!("fix every diagnostic in this file")
        );
        let edits = action_edits(
            &actions[0],
            &uri_string(site.root(), "content/document.typ"),
        );
        assert_eq!(edits.len(), 2, "{edits:?}");
        let corrected = applied_source(UNREAD_IMPORT_SOURCE, &edits);
        assert_eq!(corrected, "Body.\n");

        let unfiltered = client.code_actions("content/document.typ", &marked, &echoed, &[]);
        assert!(!unfiltered.is_empty(), "{unfiltered:?}");
        assert!(
            unfiltered
                .iter()
                .all(|action| action["kind"] != serde_json::json!("source.fixAll")),
            "{unfiltered:?}"
        );

        client.shutdown();
    }

    /// A fix-all answers for the reports the document's own check published, not only the ones the
    /// client echoed with the request.
    #[test]
    fn fix_all_covers_reports_the_client_did_not_echo() {
        let (site, mut client, reports) = unread_import_session();
        // One echoed report stands for the file; the action still fixes every report of it.
        let echoed = reports.iter().take(1).cloned().collect::<Vec<_>>();

        let marked = format!("{UNREAD_IMPORT_SOURCE}|");
        let actions =
            client.code_actions("content/document.typ", &marked, &echoed, &["source.fixAll"]);
        assert_eq!(actions.len(), 1, "{actions:?}");
        let edits = action_edits(
            &actions[0],
            &uri_string(site.root(), "content/document.typ"),
        );
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert_eq!(applied_source(UNREAD_IMPORT_SOURCE, &edits), "Body.\n");

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
    fn position_past_the_document_answers_none() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        client.open("content/document.typ", "= Title\n");

        for (line, character) in [(9999u32, 0u32), (0, 99999)] {
            let answer = client.request(
                lsp_types::request::HoverRequest::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": site.uri("content/document.typ").as_str() },
                    "position": { "line": line, "character": character },
                }),
            );
            assert!(answer.is_null(), "({line}, {character}) answered {answer}");
        }

        client.shutdown();
    }

    #[test]
    fn formatting_reads_the_unsaved_source() {
        let (site, mut client) = unsaved_document("= Saved\n", "= Title\n\n#let  unsaved =  1\n");
        let edits = client
            .formatting("content/document.typ", spaces(2))
            .expect("a replacement");
        assert!(edits[0].new_text.contains("#let unsaved = 1"), "{edits:?}");
        assert_eq!(site.read("content/document.typ"), "= Saved\n");

        client.shutdown();
    }

    #[test]
    fn range_formatting_leaves_the_rest_alone() {
        let (_site, mut client) = unsaved_document("= Saved\n", "=   Heading\n\n#let  x =  1\n");

        let edits = client
            .range_formatting(
                "content/document.typ",
                lsp_types::Range::new(
                    lsp_types::Position::new(2, 0),
                    lsp_types::Position::new(2, 12),
                ),
                spaces(2),
            )
            .expect("one edit");
        assert_eq!(edits.len(), 1, "{edits:?}");
        assert_eq!(edits[0].new_text, "#let x = 1");
        assert_eq!(edits[0].range.start.line, 2, "{edits:?}");
        assert_eq!(edits[0].range.end.line, 2, "the heading stays: {edits:?}");

        client.shutdown();
    }

    #[test]
    fn folding_reads_the_unsaved_source() {
        let (_site, mut client) = unsaved_document("Saved body.\n", "= Title\nbody\n");
        let ranges = client
            .folding("content/document.typ")
            .expect("folding ranges");
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].start_line, ranges[0].end_line), (0, 1));
        assert_eq!(ranges[0].collapsed_text.as_deref(), Some("Title"));

        client.shutdown();
    }

    #[test]
    fn outline_reads_the_unsaved_source() {
        let (_site, mut client) =
            unsaved_document("Saved body.\n", "= Title\n#let helper(x) = x\n");
        let symbols = client.symbols("content/document.typ").expect("an outline");
        let lsp_types::DocumentSymbolResponse::Nested(symbols) = symbols else {
            panic!("expected nested symbols, got {symbols:?}");
        };
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Title");
        assert_eq!(
            symbols[0].children.as_deref().map(|children| children
                .iter()
                .map(|child| child.name.as_str())
                .collect::<Vec<_>>()),
            Some(vec!["helper"])
        );

        client.shutdown();
    }

    #[test]
    fn links_read_the_unsaved_source() {
        let (site, mut client) =
            unsaved_document("Saved body.\n", "#import \"../templates/page.typ\": page\n");
        let links = client.links("content/document.typ").expect("links");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().map(|target| target.as_str()),
            Some(site.uri("templates/page.typ").as_str())
        );
        assert_eq!(links[0].range.start, lsp_types::Position::new(0, 9));
        assert_eq!(links[0].range.end, lsp_types::Position::new(0, 30));

        client.shutdown();
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
    fn enter_edits_follow_the_syntax() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);

        let edits = client
            .enter_marked("content/document.typ", "- one|\n")
            .expect("one edit");
        assert_eq!(edits.len(), 1, "{edits:?}");
        assert_eq!(edits[0].new_text, "\n- $0");

        assert_eq!(
            client.enter_marked("content/document.typ", "- |one\n"),
            None
        );

        client.shutdown();
    }

    #[test]
    fn selection_ranges_grow_from_the_cursor() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        let position = client.type_marked("content/document.typ", "#let value = (|1, 2)\n");

        let ranges = client.selection("content/document.typ", position);
        let ranges = ranges.as_array().expect("one selection");
        assert_eq!(ranges.len(), 1, "{ranges:?}");
        let mut outermost = &ranges[0];
        while let Some(parent) = outermost.get("parent") {
            outermost = parent;
        }
        assert_eq!(
            (
                outermost["range"]["start"]["line"].as_u64(),
                outermost["range"]["start"]["character"].as_u64(),
                outermost["range"]["end"]["line"].as_u64(),
                outermost["range"]["end"]["character"].as_u64(),
            ),
            (Some(0), Some(0), Some(1), Some(0)),
            "the outermost range covers the whole source: {outermost:?}"
        );

        client.shutdown();
    }

    #[test]
    fn losing_the_client_ends_the_session() {
        let site = SiteDirectory::new();
        let client = EditorSession::start(&site);
        client.abort();
    }
}
