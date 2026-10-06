//! The compiled sources one revision retains, keyed by the exact unsaved sources they came from.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tola_build::cancellation::{BuildCancellation, BuildCancelled};
use tola_build::check::{SourceDiagnosticSession, SourceRevision};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::World;

/// The compiled sources one revision retains, keyed by the exact unsaved sources they were
/// produced from.
///
/// Every request reads a revision through the unsaved sources it needs — the source it asks about
/// pinned to the text its editor holds, or a query copy with one broken construct blanked. The
/// same sources compile to the same revision however often they are asked for, so the compilation
/// outlives the request that produced it and answers the next one. A newer revision or a new
/// configuration drops every compilation held.
pub(crate) struct RevisionCompilations {
    configuration: Option<Arc<ResolvedSiteConfig>>,
    revision: u64,
    compilations: Vec<CompiledSources>,
}

/// One compilation, with the exact unsaved sources it was produced from.
struct CompiledSources {
    sources: Vec<(PathBuf, Arc<str>)>,
    compiled: Arc<SourceRevision>,
}

/// How many compilations one revision keeps: the unsaved-source sets one request cycle reads —
/// the check's pinned set, then the query's pinned, repaired, and widened ones. Retaining a
/// site's worth of worlds would be retained state out of proportion with the edits that produced
/// it, and one more set of sources is a new revision rather than another read of this one.
const RETAINED_COMPILATIONS: usize = 4;

impl RevisionCompilations {
    pub(crate) fn new() -> Self {
        Self {
            configuration: None,
            revision: 0,
            compilations: Vec::new(),
        }
    }

    /// The revision these unsaved sources produce in `revision`, compiled now unless that revision
    /// already compiled exactly them.
    ///
    /// The configuration and revision travel with the read, so no caller can ask for a
    /// compilation without stating which revision's inputs it belongs to.
    ///
    /// A read answered from a compilation held re-observes nothing: it recognizes the
    /// configuration, the revision, and the exact unsaved sources, so an input outside those —
    /// the vendor workspace, a file the client never reported as changed — stays invisible by
    /// design until a source change starts the next revision.
    pub(crate) fn inspect(
        &mut self,
        session: &mut SourceDiagnosticSession,
        configuration: &Arc<ResolvedSiteConfig>,
        revision: u64,
        sources: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<Arc<SourceRevision>, BuildCancelled> {
        cancellation.ensure_active()?;
        let adopted = self.adopt(configuration, revision);
        if adopted {
            if let Some(compiled) = self
                .compilations
                .iter()
                .find(|compiled| same_sources(&compiled.sources, &sources))
            {
                return Ok(Arc::clone(&compiled.compiled));
            }
            // A request that names the sources this revision already resolved — a query pinning a
            // file to the text the revision read, for instance — reads the revision's compilation
            // instead of deriving the site again. The world resolves every path the request names,
            // so a compilation read from these sources resolves to the same sources.
            if let Some(compiled) = self
                .compilations
                .iter()
                .find(|compiled| resolves_the_same(compiled, configuration, &sources))
            {
                return Ok(Arc::clone(&compiled.compiled));
            }
        }
        let compiled = Arc::new(session.inspect(sources.clone(), cancellation)?);
        if adopted {
            self.retain(sources, &compiled);
        }
        Ok(compiled)
    }

    /// Adopt the configuration and revision this read belongs to, and report whether the
    /// compilations held are the ones it may read.
    ///
    /// A new configuration and a newer revision both supersede what is held. An older revision
    /// reads none of it and retains none of its own: a check or a repair superseded while it ran
    /// must not drop the compilations the newest revision is answering from, and the next read of
    /// that revision finds them where they were left.
    fn adopt(&mut self, configuration: &Arc<ResolvedSiteConfig>, revision: u64) -> bool {
        let known = self
            .configuration
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, configuration));
        if !known {
            self.configuration = Some(Arc::clone(configuration));
            self.revision = revision;
            self.compilations.clear();
            return true;
        }
        match revision.cmp(&self.revision) {
            Ordering::Equal => true,
            Ordering::Greater => {
                self.revision = revision;
                self.compilations.clear();
                true
            }
            Ordering::Less => false,
        }
    }

    /// Keep one compiled revision, releasing the oldest when the bound is reached.
    fn retain(&mut self, sources: Vec<(PathBuf, Arc<str>)>, compiled: &Arc<SourceRevision>) {
        if self.compilations.len() == RETAINED_COMPILATIONS {
            self.compilations.remove(0);
        }
        self.compilations.push(CompiledSources {
            sources,
            compiled: Arc::clone(compiled),
        });
    }

    /// Let go of every compilation held; the next read re-derives what it needs.
    ///
    /// Only the compilations are released: the configuration pointer and the revision are the keys
    /// the next read compares against, and the session's own prepared resources outlive them.
    pub(crate) fn release(&mut self) {
        self.compilations.clear();
    }
}

/// Whether a held compilation answers the request it is compared against.
///
/// The request must name every unsaved source the compilation read — dropping one asks about
/// another input set — and the world that compilation built must resolve every path the request
/// names to the text the request asserts, so a pin with the text the revision already read
/// reads what is held.
fn resolves_the_same(
    compiled: &CompiledSources,
    configuration: &Arc<ResolvedSiteConfig>,
    sources: &[(PathBuf, Arc<str>)],
) -> bool {
    let Some(checked) = compiled.compiled.checked() else {
        return false;
    };
    let world = checked.world();
    let held = compiled
        .sources
        .iter()
        .map(|(path, text)| (path.as_path(), text))
        .collect::<BTreeMap<_, _>>();
    let requested = sources
        .iter()
        .map(|(path, text)| (path.as_path(), text))
        .collect::<BTreeMap<_, _>>();
    held.keys()
        .chain(requested.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .all(|path| {
            let Some(requested_text) = requested.get(path) else {
                return false;
            };
            let world_text = crate::identity::path_id(path, configuration.get_root())
                .and_then(|id| world.source(id).ok())
                .map(|source| source.text().to_owned());
            Some(requested_text.to_string()) == world_text
        })
}

/// Whether two unsaved-source sets read the same text under the same paths.
///
/// A pin or a repair rebuilds its set for every request, so the same inputs are equal paths
/// with equal text, never the same allocations.
fn same_sources(left: &[(PathBuf, Arc<str>)], right: &[(PathBuf, Arc<str>)]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|((left_path, left_text), (right_path, right_text))| {
                left_path == right_path && left_text == right_text
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::tests::{compiler, inputs_at, site_configuration};
    use crate::compiler::{CheckRequest, QueryRequest, SourceCompilation, SourceJob};
    use crate::protocol::{CheckProgress, SourceQuery};
    use crate::sources::SourceView;
    use lsp_server::RequestId;
    use std::path::Path;
    use tola_build::BuildResources;
    use tola_build::cancellation::{BuildCancellation, BuildCanceller};

    /// A request that names the sources a revision already resolved reads its compilation.
    ///
    /// A query pins the file it asks about. When that pin has the text the revision read, the
    /// compilation a fresh read would produce resolves to the same sources, so the revision's own
    /// compilation answers — with the diagnostics the fresh read would have reported.
    #[test]
    fn pinned_source_reuses_the_revision_compilation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let content = root.join("content");
        std::fs::create_dir(&content).unwrap();
        let document = content.join("document.typ");
        std::fs::write(&document, "Document body.\n").unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/document.typ\"]",
        )
        .unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"site.typ\"\n");

        let cancellation = BuildCancellation::new();
        let mut compilations = RevisionCompilations::new();
        let mut session = SourceDiagnosticSession::with_resources(
            Arc::clone(&configuration),
            BuildResources::default(),
        );
        let checked = compilations
            .inspect(&mut session, &configuration, 1, Vec::new(), &cancellation)
            .unwrap();

        let resolved: Arc<str> = Arc::from(std::fs::read_to_string(&document).unwrap());
        let pinned = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                vec![(document.clone(), Arc::clone(&resolved))],
                &cancellation,
            )
            .unwrap();
        assert!(
            Arc::ptr_eq(&checked, &pinned),
            "a pin with the text the revision read reuses its compilation"
        );
        assert_eq!(diagnostics(&pinned), diagnostics(&checked));

        // A pin with different text is a change the revision cannot answer.
        let changed: Arc<str> = Arc::from("#unknown_in_the_pinned_source\n");
        let edited = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                vec![(document.clone(), Arc::clone(&changed))],
                &cancellation,
            )
            .unwrap();
        assert!(
            !Arc::ptr_eq(&checked, &edited),
            "different text derives its own compilation"
        );

        // And the derived compilation reports what a session that never reused reports.
        let mut cold = SourceDiagnosticSession::with_resources(
            Arc::clone(&configuration),
            BuildResources::default(),
        );
        let mut cold_compilations = RevisionCompilations::new();
        let cold_revision = cold_compilations
            .inspect(
                &mut cold,
                &configuration,
                1,
                vec![(document.clone(), changed)],
                &cancellation,
            )
            .unwrap();
        assert_eq!(diagnostics(&edited), diagnostics(&cold_revision));
    }

    /// One revision's diagnostics in the shape a comparison can hold.
    fn diagnostics(revision: &SourceRevision) -> Vec<String> {
        revision
            .diagnostics()
            .iter()
            .map(|diagnostic| format!("{diagnostic:?}"))
            .collect()
    }

    /// A revision answers a query exactly as a session that never derived it does.
    ///
    /// The query pins the document it asks about. Where the pin has the text the revision read,
    /// reuse answers from the revision's compilation and a session without it derives its own — the
    /// reply has to be the same answer either way.
    #[test]
    fn reuse_answers_like_fresh_derivation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let content = root.join("content");
        std::fs::create_dir(&content).unwrap();
        let text = "= Document\n\n#let marker = 41\n\nValue: #marker\n";
        let document = content.join("document.typ");
        std::fs::write(&document, text).unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/document.typ\"]",
        )
        .unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"site.typ\"\n");

        let uri = crate::uri::from_file_path(&document).unwrap();
        let source = tola_typst::typst::syntax::Source::detached(text.to_owned());
        let cursor = text.find("#marker\n").unwrap() + "#marker".len() - 1;
        let position = crate::position::utf16_range(source.lines(), cursor..cursor)
            .unwrap()
            .start;
        let pinned: Arc<str> = Arc::from(text);
        let query = |serial: u64| {
            SourceJob::Query(QueryRequest {
                id: RequestId::from(serial as i32),
                serial,
                sources: inputs_at(
                    root,
                    1,
                    Arc::from([(document.clone(), Arc::clone(&pinned))]),
                ),
                view: SourceView::default(),
                package_sources: None,
                routes_as_hints: false,
                check_progress: CheckProgress::NotChecked,
                query: SourceQuery::Hover(lsp_types::TextDocumentPositionParams {
                    text_document: lsp_types::TextDocumentIdentifier::new(uri.clone()),
                    position,
                }),
            })
        };
        let answered = |completion: SourceCompilation| -> serde_json::Value {
            let SourceCompilation::Answered { response, .. } = completion else {
                panic!("a query is answered")
            };
            serde_json::to_value(response.expect("the query is answerable")).unwrap()
        };

        // One session derives the revision and then answers the query from it.
        let mut reused = compiler(&configuration);
        let SourceCompilation::Checked { .. } = reused.compile(SourceJob::Check(CheckRequest {
            checked_revision: 1,
            sources: inputs_at(root, 1, Arc::default()),
            view: SourceView::default(),
        })) else {
            panic!("a check is checked")
        };
        let warm = answered(reused.compile(query(2)));

        // A session that never derived the revision derives the query's own compilation.
        let mut fresh = compiler(&configuration);
        let cold = answered(fresh.compile(query(3)));

        assert_eq!(
            warm, cold,
            "a reused revision answered differently from a fresh derivation"
        );
    }

    /// A failed revision that kept a world still answers a read of exactly what that world holds.
    ///
    /// A realization failure hands back the world the site's inputs resolved, so a pin with
    /// the text that world already read reads the revision instead of deriving the same failure
    /// again — and a pin with anything else is a change the revision cannot answer.
    #[test]
    fn failed_revision_licenses_reuse_of_its_world() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        let document = root.join("content/document.typ");
        std::fs::write(&document, "Document body.\n").unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/document.typ\"\n\
             #unknown_in_the_program]\n",
        )
        .unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"site.typ\"\n");
        let cancellation = BuildCancellation::new();
        let mut compilations = RevisionCompilations::new();
        let mut session = SourceDiagnosticSession::with_resources(
            Arc::clone(&configuration),
            BuildResources::default(),
        );

        let failed = compilations
            .inspect(&mut session, &configuration, 1, Vec::new(), &cancellation)
            .unwrap();
        assert!(!failed.compiled(), "{:?}", failed.diagnostics());
        assert!(
            failed.checked().is_some(),
            "a failed realization kept the world it resolved"
        );

        let resolved: Arc<str> = Arc::from(std::fs::read_to_string(&document).unwrap());
        let answered = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                vec![(document.clone(), Arc::clone(&resolved))],
                &cancellation,
            )
            .unwrap();
        assert!(
            Arc::ptr_eq(&failed, &answered),
            "a failed revision's world answers the text it resolved"
        );

        let changed: Arc<str> = Arc::from("#unknown_in_the_pinned_source\n");
        let edited = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                vec![(document, changed)],
                &cancellation,
            )
            .unwrap();
        assert!(
            !Arc::ptr_eq(&failed, &edited),
            "text the world never resolved derives its own revision"
        );
    }

    /// A newer revision drops the compilation the previous one retained.
    ///
    /// The reused value belongs to the revision it was built from: a source change starts a new
    /// revision, whose first read must derive its own compilation, and the superseded one must not
    /// stay alive behind the cache.
    #[test]
    fn revision_bump_drops_the_retained_compilation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("content/document.typ"), "Document body.\n").unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/document.typ\"]",
        )
        .unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"site.typ\"\n");

        let cancellation = BuildCancellation::new();
        let mut compilations = RevisionCompilations::new();
        let mut session = SourceDiagnosticSession::with_resources(
            Arc::clone(&configuration),
            BuildResources::default(),
        );
        let first = compilations
            .inspect(&mut session, &configuration, 1, Vec::new(), &cancellation)
            .unwrap();
        let second = compilations
            .inspect(&mut session, &configuration, 2, Vec::new(), &cancellation)
            .unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "a newer revision derived the compilation rather than reusing a superseded one"
        );
        assert_eq!(
            Arc::strong_count(&first),
            1,
            "the superseded compilation is not kept alive by the cache"
        );
    }

    /// A site whose entry program compiles, with the configuration a compiler lane resolves.
    fn compilable_site() -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        let configuration = site_configuration(directory.path(), "");
        (directory, configuration)
    }

    /// One unsaved source of `root`, written to disk so the path names a real file.
    fn unsaved(root: &Path, name: &str, text: &str) -> Vec<(PathBuf, Arc<str>)> {
        let path = root.join(name);
        std::fs::write(&path, text).unwrap();
        vec![(path, Arc::from(text))]
    }

    #[test]
    fn reuse_follows_revision_and_sources() {
        // Every row keeps its own site: one row's read must never answer another's.
        for (first_key, second_key, expect_same) in [
            ((1, "one"), (1, "one"), true),
            ((1, "one"), (2, "one"), false),
            ((1, "one"), (1, "two"), false),
        ] {
            let (directory, configuration) = compilable_site();
            let mut compilations = RevisionCompilations::new();
            let mut session = SourceDiagnosticSession::new(Arc::clone(&configuration));
            let sources = |(revision, name): (u64, &str)| {
                (
                    revision,
                    unsaved(
                        directory.path(),
                        &format!("content/{name}.typ"),
                        &format!("= {name} <{name}>\n"),
                    ),
                )
            };
            let compiled = |compilations: &mut RevisionCompilations,
                            session: &mut SourceDiagnosticSession,
                            revision: u64,
                            sources: Vec<(PathBuf, Arc<str>)>| {
                compilations
                    .inspect(
                        session,
                        &configuration,
                        revision,
                        sources,
                        &BuildCancellation::new(),
                    )
                    .unwrap()
            };
            let (first_revision, first_sources) = sources(first_key);
            let (second_revision, second_sources) = sources(second_key);
            let first = compiled(
                &mut compilations,
                &mut session,
                first_revision,
                first_sources,
            );
            let second = compiled(
                &mut compilations,
                &mut session,
                second_revision,
                second_sources,
            );
            assert_eq!(
                Arc::ptr_eq(&first, &second),
                expect_same,
                "key {first_key:?} then {second_key:?}"
            );
        }
    }

    #[test]
    fn older_revision_keeps_the_retained_compilation() {
        let (directory, configuration) = compilable_site();
        let mut compilations = RevisionCompilations::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&configuration));
        let sources = unsaved(directory.path(), "content/one.typ", "= One <one>\n");
        let newest = compilations
            .inspect(
                &mut session,
                &configuration,
                2,
                sources.clone(),
                &BuildCancellation::new(),
            )
            .unwrap();
        // A superseded check or repair still reaching the lane must not drop what it left.
        let older = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                sources.clone(),
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(
            !Arc::ptr_eq(&newest, &older),
            "an older revision read a newer compilation"
        );
        let again = compilations
            .inspect(
                &mut session,
                &configuration,
                2,
                sources,
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(
            Arc::ptr_eq(&newest, &again),
            "an older request dropped the retained compilation"
        );
    }

    #[test]
    fn another_configuration_recompiles() {
        let (directory, configuration) = compilable_site();
        // The second site's directory guard stays alive for the whole test.
        let (_other_directory, other) = compilable_site();
        let mut compilations = RevisionCompilations::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&configuration));
        let sources = unsaved(directory.path(), "content/one.typ", "= One <one>\n");
        let first = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                sources.clone(),
                &BuildCancellation::new(),
            )
            .unwrap();
        let second = compilations
            .inspect(&mut session, &other, 1, sources, &BuildCancellation::new())
            .unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "a new configuration answered from the previous one"
        );
    }

    #[test]
    fn oldest_compilations_are_released() {
        let (directory, configuration) = compilable_site();
        let mut compilations = RevisionCompilations::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&configuration));
        let mut first = None;
        let mut last = None;
        for index in 0..=RETAINED_COMPILATIONS {
            let compiled = compilations
                .inspect(
                    &mut session,
                    &configuration,
                    1,
                    unsaved(
                        directory.path(),
                        &format!("content/{index}.typ"),
                        &format!("= Page {index} <page-{index}>\n"),
                    ),
                    &BuildCancellation::new(),
                )
                .unwrap();
            first.get_or_insert_with(|| Arc::clone(&compiled));
            last = Some(compiled);
        }
        let last = last.unwrap();
        let repeated = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                unsaved(
                    directory.path(),
                    &format!("content/{RETAINED_COMPILATIONS}.typ"),
                    &format!("= Page {RETAINED_COMPILATIONS} <page-{RETAINED_COMPILATIONS}>\n"),
                ),
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(
            Arc::ptr_eq(&last, &repeated),
            "the newest compilation was released"
        );
        let oldest = compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                unsaved(directory.path(), "content/0.typ", "= Page 0 <page-0>\n"),
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(
            !Arc::ptr_eq(&first.unwrap(), &oldest),
            "retention grew past its bound"
        );
    }

    #[test]
    fn cancelled_reuse_reports_cancellation() {
        let (directory, configuration) = compilable_site();
        let mut compilations = RevisionCompilations::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&configuration));
        let sources = unsaved(directory.path(), "content/one.typ", "= One <one>\n");
        compilations
            .inspect(
                &mut session,
                &configuration,
                1,
                sources.clone(),
                &BuildCancellation::new(),
            )
            .unwrap();
        let canceller = BuildCanceller::new();
        canceller.cancel();
        assert!(
            compilations
                .inspect(&mut session, &configuration, 1, sources, &canceller.token())
                .is_err()
        );
    }
}
