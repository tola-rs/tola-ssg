//! Watched files and source revisions: what a change to a file costs this connection, and the
//! watchers that make a file the author never opened start a check.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use lsp_server::Notification;
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, Registration,
    RegistrationParams, Unregistration, UnregistrationParams,
};
use tola_build::cancellation::BuildCanceller;
use tola_build::diagnostic::Diagnostic;

use crate::protocol::{CheckMode, CheckProgress};

use super::replies::REVISION_CHANGED_ERROR;
use super::scheduling::CHECK_SETTLE_DELAY;
use super::{Connection, PendingCheck, Phase};

/// The registration id this connection's own file watchers are known by.
const WATCHED_FILES: &str = "tola-watched-files";

/// The extensions a file can hold and still be read as a source, a configuration file, or a
/// bibliography with its style sheet: what a check that has not run yet may still read.
fn is_site_input(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| {
            matches!(extension, "typ" | "toml" | "bib" | "csl" | "yml" | "yaml")
        })
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// Whether one watched-file notification names a change this connection can answer differently
    /// for.
    ///
    /// A body this server cannot read fails open: the document handler logs it, and no filter may
    /// turn a malformed notification into silence.
    pub(super) fn watched_change_matters(&self, message: &Notification) -> bool {
        let Ok(params) = serde_json::from_value::<lsp_types::DidChangeWatchedFilesParams>(
            message.params.clone(),
        ) else {
            return true;
        };
        let Phase::Ready(workspace) = &self.phase else {
            return true;
        };
        let resolved_root = crate::uri::ClientRoot::new(&workspace.root)
            .resolved()
            .to_path_buf();
        params.changes.iter().any(|change| match change.typ {
            lsp_types::FileChangeType::CHANGED => crate::uri::to_file_path(change.uri.as_str())
                .map_or(true, |path| {
                    self.changed_file_matters(&path, &resolved_root)
                }),
            // A membership change can decide an inventory anywhere: which files a directory holds
            // is not stated by any read of one file.
            _ => true,
        })
    }

    /// Whether a changed file can change what this connection answers.
    ///
    /// The last completed check's own reads are the evidence: a file whose bytes reached an
    /// answer is one a change can reach back into. A file no check read still matters when a
    /// later one can read it — a source, a configuration file, a bibliography with its style
    /// sheet, or an ignore rule deciding which files the site holds — because a check that has not
    /// run cannot have read it yet.
    fn changed_file_matters(&self, path: &Path, root: &Path) -> bool {
        if self.read_paths.contains(path) {
            return true;
        }
        if matches!(
            path.file_name().and_then(std::ffi::OsStr::to_str),
            Some(".gitignore" | ".ignore")
        ) {
            return true;
        }
        if path.starts_with(root) && is_site_input(path) {
            return true;
        }
        let normalized = tola_build::filesystem::normalize_existing_prefix(path);
        self.read_paths.contains(&normalized)
            || (normalized.starts_with(root) && is_site_input(&normalized))
    }

    /// The newest source change becomes the newest revision, superseding every request and check
    /// before it.
    ///
    /// A typing change settles before its check runs; every other change checks at once.
    pub(super) fn sources_changed(&mut self, typing: bool) -> Result<()> {
        self.checking.cancel();
        self.cancel_selection();
        self.cancel_requests(REVISION_CHANGED_ERROR.0, REVISION_CHANGED_ERROR.1)?;
        self.queries.clear();
        self.analyses.clear();
        self.revision = self
            .revision
            .checked_add(1)
            .context("source revision counter exhausted")?;
        self.overrides = self.open.overrides();
        // One revision's reads: the next revision re-reads what it needs, so the cache cannot grow
        // with every file a long session ever reached.
        self.disk = crate::analysis::DiskSources::default();
        self.checking = BuildCanceller::new();
        let workspace = self.workspace();
        let check_mode = workspace.check_mode;
        let root = workspace.root.clone();
        let now = Instant::now();
        // A site checked on save advances its revision with every edit but checks only for a save:
        // freshness is what that mode trades away, and a check costs the whole site.
        if typing && check_mode == CheckMode::OnSave {
            self.check = None;
            self.check_due = None;
            return self.report_source_check(CheckProgress::NotChecked);
        }
        self.check_due = Some(if typing {
            now + CHECK_SETTLE_DELAY
        } else {
            now
        });
        self.check = Some(PendingCheck {
            checked_revision: self.revision,
            sources: self.source_inputs(root, self.checking.token()),
        });
        self.report_source_check(CheckProgress::Checking)
    }

    /// Ask the client to report changes below the site root, which is how a file the author never
    /// opened still starts a check.
    pub(super) fn watch_sources(&mut self) -> Result<()> {
        self.send_request(
            request::RegisterCapability::METHOD,
            RegistrationParams {
                registrations: vec![Registration {
                    id: WATCHED_FILES.to_owned(),
                    method: notification::DidChangeWatchedFiles::METHOD.to_owned(),
                    register_options: serde_json::to_value(
                        DidChangeWatchedFilesRegistrationOptions {
                            watchers: vec![FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/*".to_owned()),
                                kind: None,
                            }],
                        },
                    )
                    .ok(),
                }],
            },
        )?;
        Ok(())
    }

    /// Let the client's watchers go, so a restarting server is the one that registers them.
    pub(super) fn unwatch_sources(&mut self) -> Result<()> {
        let watched = match &self.phase {
            Phase::Ready(workspace) => workspace.features.watched_files_dynamic,
            _ => false,
        };
        if watched {
            self.send_request(
                request::UnregisterCapability::METHOD,
                UnregistrationParams {
                    unregisterations: vec![Unregistration {
                        id: WATCHED_FILES.to_owned(),
                        method: notification::DidChangeWatchedFiles::METHOD.to_owned(),
                    }],
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{
        hover_query, messages, open_document, open_draft, ready, ready_at, rejection, response,
    };
    use crate::diagnostic::UnreadImports;
    use crate::server::tests::load_configuration;
    use lsp_server::ErrorCode;
    use lsp_server::{Request, RequestId};
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tola_typst::typst::syntax::Source;

    /// A change to a file the check never read, and no later check can read as a source, a
    /// configuration, or a bibliography, costs no revision and no site-wide check.
    #[test]
    fn unread_files_outside_site_inputs_cost_no_check() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut connection = ready_at(Vec::new(), root.clone());
        connection.read_paths = [root.join("site.typ"), root.join("tola.toml")]
            .into_iter()
            .collect();

        let change = |name: &str, typ: u32| {
            Notification::new(
                notification::DidChangeWatchedFiles::METHOD.into(),
                json!({"changes":[{
                    "uri": crate::uri::from_file_path(&root.join(name)).unwrap(),
                    "type": typ,
                }]}),
            )
        };
        let _ = connection.notification(change("README.md", 2)).unwrap();
        let _ = connection.notification(change("notes.txt", 2)).unwrap();
        assert_eq!(connection.revision, 0, "an unread file changes no revision");
        assert!(connection.check.is_none());

        let _ = connection.notification(change("site.typ", 2)).unwrap();
        assert_eq!(connection.revision, 1, "a file the check read matters");
        let _ = connection
            .notification(change("content/page.typ", 2))
            .unwrap();
        assert_eq!(connection.revision, 2, "a source the site can hold matters");
        let _ = connection.notification(change("tola.toml", 2)).unwrap();
        assert_eq!(connection.revision, 3, "a configuration file matters");
        let _ = connection.notification(change("notes.txt", 1)).unwrap();
        assert_eq!(connection.revision, 4, "a membership change matters");
    }

    /// The check's read evidence decides a changed file even when the client spells the path
    /// through a symlink, and an unread file of the same kind still costs nothing.
    #[test]
    fn check_read_evidence_decides_changed_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("brand")).unwrap();
        std::fs::write(root.join("brand/logo.svg"), "<svg></svg>").unwrap();
        std::fs::write(root.join("brand/unread.svg"), "<svg></svg>").unwrap();
        let mut connection = ready_at(Vec::new(), root.clone());
        connection.read_paths = [root.join("brand/logo.svg")].into_iter().collect();

        let change = |path: PathBuf| {
            Notification::new(
                notification::DidChangeWatchedFiles::METHOD.into(),
                json!({"changes":[{
                    "uri": crate::uri::from_file_path(&path).unwrap(),
                    "type": 2,
                }]}),
            )
        };
        let _ = connection
            .notification(change(root.join("brand/logo.svg")))
            .unwrap();
        assert_eq!(connection.revision, 1, "an asset the check read matters");

        // The client's own spelling of the root reaches the same file.
        let _ = connection
            .notification(change(directory.path().join("brand/logo.svg")))
            .unwrap();
        assert_eq!(connection.revision, 2, "a symlinked spelling still matters");

        let _ = connection
            .notification(change(root.join("brand/unread.svg")))
            .unwrap();
        assert_eq!(connection.revision, 2, "an unread asset changes nothing");
    }

    #[test]
    fn source_change_drops_pending_requests() {
        let mut connection = ready(Vec::new());
        let id = RequestId::from(9);
        connection.query(id.clone(), hover_query()).unwrap();
        connection.sources_changed(false).unwrap();
        assert_eq!(
            rejection(&mut connection, &id),
            ErrorCode::ContentModified as i32
        );
    }

    /// A file-change notification for generated state must not supersede a query that the site's
    /// own sources still answer.
    #[test]
    fn generated_events_preserve_pending_queries() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let uri = crate::uri::from_file_path(&directory.path().join("page.typ")).unwrap();
        let text = "#let count = 1\n#count";
        open_document(&mut connection, &uri, 1, text);
        let source = Source::detached(text);
        let byte = text.rfind("count").unwrap();
        let position = crate::position::utf16_range(source.lines(), byte..byte)
            .unwrap()
            .start;
        connection
            .request(Request::new(
                9.into(),
                request::References::METHOD.into(),
                json!({
                    "textDocument":{"uri":uri.as_str()}, "position":position, "context":{"includeDeclaration":true}
                }),
            ))
            .unwrap();
        let pending = connection
            .next_analysis()
            .expect("source analysis admission");
        let generated =
            crate::uri::from_file_path(&directory.path().join(".tola/page.typ")).unwrap();
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeWatchedFiles::METHOD.into(),
                json!({"changes":[{"uri":generated.as_str(),"type":1}]}),
            ))
            .unwrap();
        connection
            .completed(crate::compiler::analyze(
                &mut crate::analysis::DiskSources::default(),
                &mut crate::analysis::GraphCache::default(),
                pending,
            ))
            .unwrap();
        let locations: Vec<lsp_types::Location> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        let expected: Vec<_> = text
            .match_indices("count")
            .map(|(start, name)| {
                lsp_types::Location::new(
                    uri.clone(),
                    crate::position::utf16_range(source.lines(), start..start + name.len())
                        .unwrap(),
                )
            })
            .collect();
        assert_eq!(locations, expected);
    }

    /// A draft holds no site path, so its own edits are no revision of the site's sources: the
    /// index a check of them returned keeps answering, and the answer it gives does not move.
    #[test]
    fn draft_edit_keeps_the_checked_selection() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("content/helpers.typ"), "#let orphan = 42\n").unwrap();
        let uri = crate::uri::from_file_path(&root.join("content/page.typ")).unwrap();
        let text = "#import \"helpers.typ\": orphan\nBody.\n";
        std::fs::write(root.join("content/page.typ"), text).unwrap();
        let mut connection = ready_at(Vec::new(), root.to_path_buf());
        open_document(&mut connection, &uri, 1, text);
        let mut compiler = crate::compiler::SourceCompiler::new(
            |root: &Path, _: &[(PathBuf, Arc<str>)]| load_configuration(root),
            tola_build::BuildResources::default(),
        );
        let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
        connection.completed(checked).unwrap();
        messages(&mut connection);
        let revision = connection.revision;

        let draft = open_draft(
            &mut connection,
            "#import \"content/helpers.typ\": orphan\nDraft body.\n",
        );
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeTextDocument::METHOD.into(),
                json!({
                    "textDocument":{"uri":draft.as_str(),"version":2},
                    "contentChanges":[{"text":"#import \"content/helpers.typ\": orphan\nDraft body, edited.\n"}]
                }),
            ))
            .unwrap();
        assert_eq!(
            connection.revision, revision,
            "a draft is no site source, so its edit starts no revision"
        );

        let source = connection.source(&uri).unwrap();
        let Phase::Ready(workspace) = &mut connection.phase else {
            unreachable!()
        };
        let UnreadImports::Answered(diagnostics) = workspace
            .diagnostics
            .unused_imports(revision, root, &connection.open, &uri, &source)
            .unwrap()
        else {
            panic!("the check's own index still answers for this revision");
        };
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    }
}
