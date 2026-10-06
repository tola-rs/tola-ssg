//! The check job: one revision's diagnostics, read evidence, and selection index.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{SourceDiagnosticSession, SourceRevision};
use tola_build::config::ResolvedSiteConfig;
use tola_build::diagnostic::{Diagnostic, Severity};
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::{SelectedInterfaces, SourceNames};

use super::jobs::{SourceInputs, UnsavedSource};
use super::worker::{SourceCompiler, SourceFailure};

use crate::analysis::DiskSources;
use crate::server::ServedWorkspace;
use crate::sources::SourceView;

pub(crate) struct CheckedSources {
    pub(crate) configuration: Option<Arc<ResolvedSiteConfig>>,
    pub(crate) compiled: bool,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// The disk paths this check read, or `None` when it established no read evidence: a consumer
    /// keeps the paths it had rather than reading the check as having read nothing.
    pub(crate) read_paths: Option<Vec<PathBuf>>,
    /// What every source of this site selects from every other, as this check read them.
    ///
    /// The check builds it before its own liveness pass reads it, and the connection keeps it for
    /// the revision this check answers: the report that revision publishes reads the same index
    /// the findings were licensed against.
    pub(crate) selected: Arc<SelectedInterfaces>,
}

/// The root a check's own report renders paths and resolves the site through: the configuration it
/// resolved, or the workspace root when it resolved none.
///
/// The selection index a check builds is built under the same root, so a report reads exactly the
/// index its own check returned.
pub(crate) fn checked_root<'a>(
    configuration: Option<&'a Arc<ResolvedSiteConfig>>,
    workspace_root: &'a Path,
) -> &'a Path {
    configuration.map_or(workspace_root, |configuration| configuration.get_root())
}

/// The selection index one check licenses its findings with, built for the revision it reads.
///
/// The connection never builds one: the whole-site walk belongs to the check's own lane, where a
/// check the author has already left behind stops it.
fn checked_selection(
    sources: &SourceInputs,
    view: &SourceView,
    configuration: Option<&Arc<ResolvedSiteConfig>>,
    disk: &mut DiskSources,
) -> Result<Arc<SelectedInterfaces>, SourceFailure> {
    crate::analysis::site_selected_interfaces(
        checked_root(configuration, &sources.root),
        view,
        disk,
        &sources.cancellation,
    )
    .map_err(SourceFailure::of)
}

impl<F> SourceCompiler<F>
where
    F: FnMut(&Path, &[UnsavedSource]) -> Result<ServedWorkspace>,
{
    pub(super) fn check(
        &mut self,
        sources: &SourceInputs,
        view: &SourceView,
    ) -> Result<CheckedSources, SourceFailure> {
        if let Err(error) = self.prepare(sources).map(|_| ()) {
            return match SourceFailure::of(error) {
                failure @ SourceFailure::Cancelled => Err(failure),
                SourceFailure::Failed(error) => {
                    // A broken edit retains the last usable workspace configuration.
                    let configuration = self
                        .session
                        .as_ref()
                        .map(|(served, _)| Arc::clone(served.configuration()));
                    Ok(CheckedSources {
                        selected: checked_selection(
                            sources,
                            view,
                            configuration.as_ref(),
                            &mut self.disk,
                        )?,
                        configuration,
                        compiled: false,
                        diagnostics: configuration_diagnostics(error),
                        // A failed load established no read evidence; the paths the last completed
                        // check read still stand.
                        read_paths: None,
                    })
                }
            };
        }
        let (served, session) = Self::prepared_session(&mut self.session);
        let configuration = Arc::clone(served.configuration());
        let selected = checked_selection(sources, view, Some(&configuration), &mut self.disk)?;
        if !served.compiles_site() {
            return check_documents(session, &configuration, sources, view, &selected);
        }
        let revision = self
            .compilations
            .inspect(
                session,
                &configuration,
                sources.source_revision,
                sources.overrides.to_vec(),
                &sources.cancellation,
            )
            .map_err(SourceFailure::from)?;
        let compiled = revision.compiled();
        let mut diagnostics = revision.diagnostics().to_vec();
        close_editor_revision(session, &configuration, &mut diagnostics);
        // Editor hints ride the check: they need the same world the compiler read.
        let world = revision.checked().map(|checked| checked.world());
        for (path, text) in sources.overrides.iter() {
            let Some(names) = open_document_names(view, path, text, configuration.get_root())
            else {
                continue;
            };
            append_document_editor_diagnostics(
                &mut diagnostics,
                path,
                configuration.get_root(),
                &names,
                world,
                &selected,
                &sources.cancellation,
            )?;
        }
        sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        Ok(CheckedSources {
            selected,
            configuration: Some(configuration),
            compiled,
            diagnostics,
            read_paths: disk_reads(&revision),
        })
    }
}

/// The parsed index of one open document, under the id the site's selection index knows it by.
///
/// Only a Typst document inside the workspace's own root is one the index can license. The
/// connection parsed every document it holds for its own answers, so a snapshot this revision
/// already indexed under the text the check reads answers here; only a document the connection
/// holds no index for is parsed from the text.
fn open_document_names(
    view: &SourceView,
    path: &Path,
    text: &str,
    root: &Path,
) -> Option<Arc<SourceNames>> {
    if path.extension().and_then(OsStr::to_str) != Some("typ") {
        return None;
    }
    let id = crate::identity::path_id(path, root)?;
    match view.by_id(id) {
        Some(snapshot) if snapshot.source.text() == text => Some(snapshot.names()),
        _ => Some(Arc::new(SourceNames::new(Source::new(id, text.to_owned())))),
    }
}

/// Append the hints and liveness findings one open document's own index justifies, in source
/// order.
///
/// Hints are measured against the diagnostics reported before this call, so only a diagnostic that
/// already covers a hint's range drops it. Liveness is source-local: it needs no compiled world,
/// only the selection evidence frozen with this check's sources.
fn append_document_editor_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    path: &Path,
    root: &Path,
    names: &SourceNames,
    world: Option<&tola_typst::TypstWorld>,
    selected: &SelectedInterfaces,
    cancellation: &BuildCancellation,
) -> Result<(), SourceFailure> {
    let start = diagnostics.len();
    let hints = crate::lint::document_diagnostics(path, root, names, world, &diagnostics[..start]);
    diagnostics.extend(hints);
    let Some(liveness) = names.liveness(selected, || cancellation.is_cancelled()) else {
        return Err(SourceFailure::Cancelled);
    };
    diagnostics.extend(crate::lint::liveness_diagnostics(
        path, root, names, &liveness,
    ));
    diagnostics[start..].sort_by_key(|diagnostic| {
        diagnostic
            .location
            .as_ref()
            .and_then(|location| location.range.as_ref())
            .map(|range| (range.start.line, range.start.character))
    });
    Ok(())
}

/// Close one check's editor revision: advance the parsed-file window, then report the resolved
/// configuration's warnings.
///
/// The parsed-file window advances only when a check ends its editor revision; a query compilation
/// must not age it, or a hover burst would drop sources the site still holds.
fn close_editor_revision(
    session: &mut SourceDiagnosticSession,
    configuration: &ResolvedSiteConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    session.evict_stale_file_cache_entries();
    diagnostics.extend(tola_build::config::diagnostic::warning_diagnostics(
        configuration,
    ));
}

/// Check each document a workspace holds open in the world its own imports resolve.
///
/// A workspace that holds no site configuration compiles no site: what the author reads is what
/// each document establishes alone, in the same world a query about a file the site's program
/// never reached answers from.
fn check_documents(
    session: &mut SourceDiagnosticSession,
    configuration: &Arc<ResolvedSiteConfig>,
    sources: &SourceInputs,
    view: &SourceView,
    selected: &Arc<SelectedInterfaces>,
) -> Result<CheckedSources, SourceFailure> {
    let overrides = sources.overrides.to_vec();
    let mut diagnostics = Vec::new();
    let mut compiled = false;
    let mut read_paths = Vec::new();
    for (path, text) in &overrides {
        // A document outside the workspace's own directory belongs to another root, and a vendor
        // file is not the author's document.
        let Some(relative) = path
            .strip_prefix(configuration.get_root())
            .ok()
            .and_then(Path::to_str)
        else {
            continue;
        };
        let Some(names) = open_document_names(view, path, text, configuration.get_root()) else {
            continue;
        };
        let revision = session
            .inspect_source(relative, overrides.clone(), &sources.cancellation)
            .map_err(SourceFailure::from)?;
        // A document that resolved a world answers, even when its own file did not compile; a check
        // that resolved none leaves the revision without a usable world.
        compiled |= revision.checked().is_some();
        diagnostics.extend_from_slice(revision.diagnostics());
        append_document_editor_diagnostics(
            &mut diagnostics,
            path,
            configuration.get_root(),
            &names,
            revision.checked().map(|checked| checked.world()),
            selected,
            &sources.cancellation,
        )?;
        read_paths.extend(disk_reads(&revision).unwrap_or_default());
    }
    close_editor_revision(session, configuration, &mut diagnostics);
    sources
        .cancellation
        .ensure_active()
        .map_err(SourceFailure::from)?;
    Ok(CheckedSources {
        selected: Arc::clone(selected),
        configuration: Some(Arc::clone(configuration)),
        compiled,
        diagnostics,
        read_paths: Some(read_paths),
    })
}

/// The disk paths one revision read, as the evidence a file-change filter compares against.
///
/// The evidence is the compiling revision's own: the site's root Bundle, or one source compiled
/// without the root program. A revision that reached no compilation — a realization that failed
/// kept a world but compiled nothing — establishes none, and reporting its empty record as "read
/// nothing" would drop what the last complete check read, so a change to a file only that check
/// read would stop costing a revision. A read served by a provider or a process-local source names
/// no path a client can report, so it contributes none.
fn disk_reads(revision: &SourceRevision) -> Option<Vec<PathBuf>> {
    if !revision.compiled() {
        return None;
    }
    let compilation = revision.checked()?;
    Some(
        compilation
            .file_reads()
            .iter()
            .filter_map(|read| match read.origin() {
                tola_typst::ReadOrigin::Disk(path) => Some(path.as_path().to_path_buf()),
                _ => None,
            })
            .collect(),
    )
}

fn configuration_diagnostics(error: anyhow::Error) -> Vec<Diagnostic> {
    if let Some(attached) = tola_build::diagnostic::attached(&error) {
        return attached.to_vec();
    }
    let fallback = tola_build::diagnostic::fallback(crate::codes::editor::CONFIGURATION, &error);
    vec![Diagnostic::at_path(
        crate::codes::editor::CONFIGURATION,
        Severity::Error,
        "tola.toml",
        fallback.message,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::tests::{compiler, inputs_at, site_configuration};
    use crate::compiler::{CheckRequest, SourceCompilation, SourceCompiler, SourceJob};

    /// An open document's unused bindings reach the check's diagnostics.
    #[test]
    fn check_reports_open_document_liveness() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let content = root.join("content");
        std::fs::create_dir(&content).unwrap();
        let text = "#let unused = 1\n#let used = 2\n#used\n";
        let document = content.join("page.typ");
        std::fs::write(&document, text).unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/page.typ\"]",
        )
        .unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"site.typ\"\n");
        let mut compiler = compiler(&configuration);
        let SourceCompilation::Checked { checked, .. } =
            compiler.compile(SourceJob::Check(CheckRequest {
                checked_revision: 1,
                sources: inputs_at(
                    root,
                    1,
                    Arc::from([(document.clone(), Arc::<str>::from(text))]),
                ),
                view: SourceView::default(),
            }))
        else {
            panic!("expected check completion")
        };
        let checked = checked.unwrap();
        assert!(
            checked.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == crate::codes::editor::UNUSED_BINDING
                    && diagnostic.message == "the binding `unused` is never read"
            }),
            "{:?}",
            checked.diagnostics
        );
    }

    /// A check states its read evidence only while its root program compiles.
    ///
    /// A failed realization keeps the world it prepared but records no reads, so reporting that
    /// empty record would drop the paths the last complete check read: a change to a file only
    /// that check read — the data file below, which is no source — would stop costing a revision,
    /// and the failed revision's diagnostics would stand over inputs that changed.
    #[test]
    fn failed_check_states_no_read_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("data.txt"), "payload\n").unwrap();
        std::fs::write(root.join("content/document.typ"), "= Body\n").unwrap();
        let program = |body: &str| {
            format!(
                "#import \"@tola/source:0.0.0\": all-sources\n\
                 #let extra = read(\"/data.txt\")\n\
                 #for source in all-sources() {{\n\
                 \x20 document(source.id, format: \"html\")[{body}#extra\n#include source.file]\n\
                 }}\n"
            )
        };
        let configuration = site_configuration(root, "");
        let mut compiler = compiler(&configuration);
        let check = |compiler: &mut SourceCompiler<_>, revision: u64, body: &str| {
            std::fs::write(root.join("site.typ"), program(body)).unwrap();
            let SourceCompilation::Checked { checked, .. } =
                compiler.compile(SourceJob::Check(CheckRequest {
                    checked_revision: revision,
                    sources: inputs_at(root, revision, Arc::default()),
                    view: SourceView::default(),
                }))
            else {
                panic!("expected check completion")
            };
            checked.unwrap()
        };

        let compiled = check(&mut compiler, 1, "");
        assert!(compiled.compiled, "{:?}", compiled.diagnostics);
        let reads = compiled.read_paths.expect("a compiled check reads files");
        assert!(
            reads.iter().any(|path| path.ends_with("data.txt")),
            "{reads:?}"
        );

        let failed = check(&mut compiler, 2, "#unknown_in_the_program\n");
        assert!(!failed.compiled, "{:?}", failed.diagnostics);
        assert!(
            failed.read_paths.is_none(),
            "a failed check established no read evidence: {:?}",
            failed.read_paths
        );
    }

    #[test]
    fn unsaved_entry_is_compiled_in_place() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let content = root.join("content");
        std::fs::create_dir(&content).unwrap();
        let entry = content.join("site.typ");
        std::fs::write(&entry, "#document(\"saved.html\")[Saved entry]").unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"content/site.typ\"\n");
        let mut compiler = compiler(&configuration);
        let program = r#"#import "@tola/address:0.0.0": route
#import "@tola/source:0.0.0": all-sources
#let sources = all-sources()
#assert.eq(sources.map(source => source.id), ("document.typ",))
#assert.eq(route(sources.first().route-segments), "/document/")
#document("document/index.html")[#include sources.first().file]"#;
        let document: Arc<str> = Arc::from(
            "#import \"@tola/address:0.0.0\": route\n\
             #import \"@tola/source:0.0.0\": current-source\n\
             #let input = current-source()\n\
             #assert.eq(input.id, \"document.typ\")\n\
             #assert.eq(route(input.route-segments), \"/document/\")\n\
             Unsaved document.",
        );
        for (revision, text) in [
            (1, Arc::<str>::from(program)),
            (2, Arc::<str>::from("#unknown_in_unsaved_entry")),
        ] {
            let SourceCompilation::Checked { checked, .. } =
                compiler.compile(SourceJob::Check(CheckRequest {
                    checked_revision: revision,
                    sources: inputs_at(
                        root,
                        revision,
                        Arc::from([
                            (entry.clone(), text),
                            (content.join("document.typ"), Arc::clone(&document)),
                        ]),
                    ),
                    view: SourceView::default(),
                }))
            else {
                panic!("expected check completion");
            };
            let checked = checked.unwrap();
            if revision == 1 {
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .all(|diagnostic| diagnostic.severity != Severity::Error),
                    "{:?}",
                    checked.diagnostics
                );
            } else {
                assert!(checked.diagnostics.iter().any(|diagnostic| {
                    diagnostic.severity == Severity::Error
                        && diagnostic.message.contains("unknown_in_unsaved_entry")
                }));
            }
        }
        assert_eq!(
            std::fs::read_to_string(entry).unwrap(),
            "#document(\"saved.html\")[Saved entry]"
        );
        assert!(!content.join("document.typ").exists());
        assert!(!configuration.build().publish_dir.exists());
    }
}
