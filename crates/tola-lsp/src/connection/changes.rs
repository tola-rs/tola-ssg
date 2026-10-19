//! Watched files and source revisions: what a change to a file costs this connection, and the
//! watchers that make a file the author never opened start a check.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use lsp_server::Notification;
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, OneOf, Registration,
    RegistrationParams, RelativePattern, Unregistration, UnregistrationParams,
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
        let Phase::Ready(_) = &self.phase else {
            return true;
        };
        let resolved_root = self.source_root();
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
        // Only a successful check of this revision can exclude unread files: a pending or failed
        // check may have reached dependencies absent from the previous revision's reads.
        let incomplete_reads = self.checked != Some((self.revision, true));
        if path.starts_with(root) && (is_site_input(path) || incomplete_reads) {
            return true;
        }
        let normalized = tola_build::filesystem::normalize_existing_prefix(path);
        self.read_paths.contains(&normalized)
            || (normalized.starts_with(root) && (is_site_input(&normalized) || incomplete_reads))
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
        let Phase::Ready(workspace) = &self.phase else {
            return Ok(());
        };
        if !workspace.features.watched_files_dynamic {
            return Ok(());
        }
        let relative = workspace.features.watched_files_relative;
        let client_root = crate::uri::ClientRoot::new(&workspace.root);
        let roots = self.input_roots();
        if self.watched_roots == roots {
            return Ok(());
        }
        let mut watchers = Vec::with_capacity(roots.len());
        for root in &roots {
            let glob_pattern = if relative {
                GlobPattern::Relative(RelativePattern {
                    base_uri: OneOf::Right(client_root.address(root)?),
                    pattern: "**/*".to_owned(),
                })
            } else {
                GlobPattern::String(source_glob(&client_root.spell(root)))
            };
            watchers.push(FileSystemWatcher {
                glob_pattern,
                kind: None,
            });
        }
        self.unwatch_sources()?;
        self.send_request(
            request::RegisterCapability::METHOD,
            RegistrationParams {
                registrations: vec![Registration {
                    id: WATCHED_FILES.to_owned(),
                    method: notification::DidChangeWatchedFiles::METHOD.to_owned(),
                    register_options: serde_json::to_value(
                        DidChangeWatchedFilesRegistrationOptions { watchers },
                    )
                    .ok(),
                }],
            },
        )?;
        self.watched_roots = roots;
        Ok(())
    }

    /// Let the client's watchers go, so a restarting server is the one that registers them.
    pub(super) fn unwatch_sources(&mut self) -> Result<()> {
        if !self.watched_roots.is_empty() {
            self.send_request(
                request::UnregisterCapability::METHOD,
                UnregistrationParams {
                    unregisterations: vec![Unregistration {
                        id: WATCHED_FILES.to_owned(),
                        method: notification::DidChangeWatchedFiles::METHOD.to_owned(),
                    }],
                },
            )?;
            self.watched_roots.clear();
        }
        Ok(())
    }

    /// The directories a check can read: the effective source root, then every package root the
    /// invocation declared outside it.
    fn input_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.source_root()];
        if let Some(locations) = self
            .configuration
            .as_ref()
            .map(|config| config.package_locations())
        {
            for location in locations.declared() {
                let root = tola_build::filesystem::normalize_existing_prefix(location.root());
                if root.is_dir() && !roots.iter().any(|watched| root.starts_with(watched)) {
                    roots.push(root);
                }
            }
        }
        roots
    }
}

fn source_glob(root: &Path) -> String {
    let path = root
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let mut pattern = String::with_capacity(path.len() + 5);
    for character in path.chars() {
        if matches!(character, '[' | ']' | '{' | '}' | '*' | '?') {
            pattern.push('[');
            pattern.push(character);
            pattern.push(']');
        } else {
            pattern.push(character);
        }
    }
    if !pattern.ends_with('/') {
        pattern.push('/');
    }
    pattern.push_str("**/*");
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{
        hover_query, messages, open_document, open_draft, ready, ready_at, ready_workspace,
        rejection, response,
    };
    use crate::diagnostic::UnreadImports;
    use crate::server::tests::load_configuration;
    use lsp_server::{ErrorCode, Message, Request, RequestId};
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tola_typst::typst::syntax::Source;

    #[test]
    fn watchers_follow_effective_source_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let workspace = root.join("workspace");
        let site = root.join("site");
        let next = root.join("next");
        for directory in [&workspace, &site, &next] {
            std::fs::create_dir(directory).unwrap();
        }
        for relative in [false, true] {
            let mut connection = ready_at(Vec::new(), workspace.clone());
            connection.named_configuration = Some(site.join("tola.toml"));
            ready_workspace(&mut connection).features = crate::capabilities::ClientFeatures::new(
                &serde_json::from_value(json!({"workspace":{"didChangeWatchedFiles":{
                    "dynamicRegistration":true,"relativePatternSupport":relative
                }}}))
                .unwrap(),
            );
            connection.watch_sources().unwrap();
            let written = messages(&mut connection);
            let [Message::Request(registration)] = written.as_slice() else {
                panic!("one watcher registration: {written:?}");
            };
            let pattern = &registration.params["registrations"][0]["registerOptions"]["watchers"]
                [0]["globPattern"];
            if relative {
                assert_eq!(
                    pattern["baseUri"],
                    crate::uri::from_file_path(&site).unwrap().as_str()
                );
                assert_eq!(pattern["pattern"], "**/*");
            } else {
                let spelling = site
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                assert_eq!(pattern, &json!(format!("{spelling}/**/*")));
            }
            connection.watch_sources().unwrap();
            assert!(messages(&mut connection).is_empty());
            connection.named_configuration = Some(next.join("tola.toml"));
            connection.watch_sources().unwrap();
            let written = messages(&mut connection);
            assert_eq!(written.len(), 2);
            assert!(
                matches!(&written[0], Message::Request(request) if request.method == request::UnregisterCapability::METHOD)
            );
            assert!(
                matches!(&written[1], Message::Request(request) if request.method == request::RegisterCapability::METHOD)
            );
        }
    }

    /// A package root the invocation declares outside the site is watched with it, while a root
    /// inside the site is already covered and registers nothing extra.
    #[test]
    fn watchers_include_declared_package_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let site = root.join("site");
        let vendored = root.join("vendored");
        for directory in [&site, &vendored, &site.join("vendor")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(site.join("tola.toml"), "").unwrap();
        let packages = tola_typst::PackageLocations::from_absolute_roots(None, None)
            .unwrap()
            .with_declared_root(vendored.clone())
            .unwrap()
            .with_declared_root(site.join("vendor"))
            .unwrap();
        let config = tola_build::config::loading::load_site_config(
            Some(&site.join("tola.toml")),
            packages,
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let mut connection = ready_at(Vec::new(), site.clone());
        connection.configuration = Some(Arc::new(config));
        ready_workspace(&mut connection).features = crate::capabilities::ClientFeatures::new(
            &serde_json::from_value(json!({"workspace":{"didChangeWatchedFiles":{
                "dynamicRegistration":true,"relativePatternSupport":true
            }}}))
            .unwrap(),
        );
        connection.watch_sources().unwrap();
        let written = messages(&mut connection);
        let [Message::Request(registration)] = written.as_slice() else {
            panic!("one watcher registration: {written:?}");
        };
        let watchers = &registration.params["registrations"][0]["registerOptions"]["watchers"];
        let bases = watchers
            .as_array()
            .unwrap()
            .iter()
            .map(|watcher| watcher["globPattern"]["baseUri"].clone())
            .collect::<Vec<_>>();
        assert_eq!(bases.len(), 2, "{watchers:?}");
        assert_eq!(
            bases[0],
            json!(crate::uri::from_file_path(&site).unwrap().as_str())
        );
        assert_eq!(
            bases[1],
            json!(crate::uri::from_file_path(&vendored).unwrap().as_str())
        );
        connection.watch_sources().unwrap();
        assert!(messages(&mut connection).is_empty());
    }

    #[test]
    fn static_watchers_add_no_registration() {
        let mut connection = ready(Vec::new());
        connection.watch_sources().unwrap();
        assert!(messages(&mut connection).is_empty());
        assert!(connection.watched_roots.is_empty());
    }

    #[test]
    fn watcher_globs_escape_root_metacharacters() {
        assert_eq!(
            source_glob(Path::new("/site[a]{b}*?")),
            "/site[[]a[]][{]b[}][*][?]/**/*"
        );
        assert_eq!(source_glob(Path::new("/")), "/**/*");
    }

    #[cfg(windows)]
    #[test]
    fn watcher_globs_use_forward_separators() {
        assert_eq!(
            source_glob(Path::new(r"C:\site[one]\content")),
            "C:/site[[]one[]]/content/**/*"
        );
    }

    #[test]
    fn failed_data_change_starts_check() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("tola.toml"), "").unwrap();
        std::fs::write(root.join("data.json"), "invalid").unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#let value = json(\"data.json\")\n#document(\"index.html\", format: \"html\")[Hello]",
        )
        .unwrap();
        let mut connection = ready_at(Vec::new(), root.clone());
        let mut compiler = crate::compiler::SourceCompiler::new(
            |root: &Path, _: &[(PathBuf, Arc<str>)]| load_configuration(root),
            tola_build::BuildResources::default(),
        );
        connection.sources_changed(false).unwrap();
        let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
        connection.completed(checked).unwrap();
        assert_eq!(connection.checked, Some((connection.revision, false)));

        let change = |name: &str| {
            Notification::new(
                notification::DidChangeWatchedFiles::METHOD.into(),
                json!({"changes":[{
                    "uri": crate::uri::from_file_path(&root.join(name)).unwrap(),
                    "type": 2,
                }]}),
            )
        };
        std::fs::write(root.join("data.json"), "{}").unwrap();
        let _ = connection.notification(change("data.json")).unwrap();
        let pending_revision = connection.revision;
        std::fs::write(root.join("data.json"), "{\"value\": 1}").unwrap();
        let _ = connection.notification(change("data.json")).unwrap();
        assert!(connection.revision > pending_revision);
        let checked = compiler.compile(
            connection
                .next_job(Instant::now())
                .expect("the corrected JSON starts a check"),
        );
        connection.completed(checked).unwrap();
        assert_eq!(connection.checked, Some((connection.revision, true)));

        let revision = connection.revision;
        std::fs::write(root.join("unread.bin"), [0, 1, 2]).unwrap();
        let _ = connection.notification(change("unread.bin")).unwrap();
        assert_eq!(connection.revision, revision);
        assert!(connection.next_job(Instant::now()).is_none());
    }

    #[test]
    fn unchecked_data_changes_supersede_checks() {
        for had_successful_check in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            std::fs::create_dir(root.join("content")).unwrap();
            std::fs::write(root.join("tola.toml"), "").unwrap();
            std::fs::write(root.join("data.json"), "{\"ok\": false}").unwrap();
            let entry = root.join("site.typ");
            let body = "#document(\"index.html\", format: \"html\")[Hello]";
            let mut connection = ready_at(Vec::new(), root.clone());
            let mut compiler = crate::compiler::SourceCompiler::new(
                |root: &Path, _: &[(PathBuf, Arc<str>)]| load_configuration(root),
                tola_build::BuildResources::default(),
            );
            let change = |name: &str| {
                Notification::new(
                    notification::DidChangeWatchedFiles::METHOD.into(),
                    json!({"changes":[{
                        "uri": crate::uri::from_file_path(&root.join(name)).unwrap(),
                        "type": 2,
                    }]}),
                )
            };
            if had_successful_check {
                std::fs::write(&entry, body).unwrap();
                connection.sources_changed(false).unwrap();
                let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
                connection.completed(checked).unwrap();
                assert_eq!(connection.checked, Some((connection.revision, true)));
                assert!(!connection.read_paths.contains(&root.join("data.json")));
            }
            std::fs::write(
                &entry,
                format!("#let value = json(\"data.json\")\n#assert.eq(value.ok, true)\n{body}"),
            )
            .unwrap();
            let _ = connection.notification(change("site.typ")).unwrap();
            let pending_revision = connection.revision;
            let pending = compiler.compile(connection.next_job(Instant::now()).unwrap());
            assert!(matches!(
                &pending,
                crate::compiler::SourceCompilation::Checked { checked: Ok(checked), .. }
                    if !checked.compiled && !checked.diagnostics.is_empty()
            ));
            messages(&mut connection);

            std::fs::write(root.join("data.json"), "{\"ok\": true}").unwrap();
            let _ = connection.notification(change("data.json")).unwrap();
            assert!(connection.revision > pending_revision);
            connection.completed(pending).unwrap();
            assert!(messages(&mut connection).is_empty());
            assert!(
                !connection
                    .checked
                    .is_some_and(|(revision, _)| revision == pending_revision)
            );

            let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
            connection.completed(checked).unwrap();
            assert_eq!(connection.checked, Some((connection.revision, true)));
        }
    }

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
        connection.checked = Some((connection.revision, true));

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
        connection.checked = Some((connection.revision, true));

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
            .notification(change(root.join("brand/unread.svg")))
            .unwrap();
        assert_eq!(connection.revision, 0, "an unread asset changes nothing");
        let _ = connection
            .notification(change(root.join("brand/logo.svg")))
            .unwrap();
        assert_eq!(connection.revision, 1, "an asset the check read matters");

        // The client's own spelling of the root reaches the same file.
        let _ = connection
            .notification(change(directory.path().join("brand/logo.svg")))
            .unwrap();
        assert_eq!(connection.revision, 2, "a symlinked spelling still matters");
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
