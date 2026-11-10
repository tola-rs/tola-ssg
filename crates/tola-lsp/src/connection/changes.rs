//! Watched files and source revisions: what a change to a file costs this connection, and the
//! watchers that make a file the author never opened start a check.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use lsp_server::{Notification, RequestId, Response};
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, OneOf, Registration,
    RegistrationParams, RelativePattern, Unregistration, UnregistrationParams,
};
use tola_build::cancellation::BuildCanceller;
use tola_build::config::ResolvedSiteConfig;
use tola_diagnostic::Diagnostic;
use tola_input::normalize_existing_prefix;
use tola_typst::PackageLocationSource;

use crate::protocol::{CheckMode, CheckProgress};

use super::replies::REVISION_CHANGED_ERROR;
use super::scheduling::CHECK_SETTLE_DELAY;
use super::{Connection, PendingCheck, Phase};

#[derive(Default)]
pub(super) struct Watchers {
    registered: Option<WatchRegistration>,
    pending: Option<(RequestId, WatchRegistration)>,
    /// Retry refused paths on a later revision, rather than on every completed query.
    refused: Option<(u64, Vec<PathBuf>)>,
}

struct WatchRegistration {
    id: String,
    paths: Vec<PathBuf>,
}

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
    fn changed_file_matters(&self, reported: &Path, root: &Path) -> bool {
        let physical = normalize_existing_prefix(reported);
        if self.configuration_path(root) == physical || physical == root.join("tola.lock") {
            return true;
        }
        if self.configuration.as_ref().is_some_and(|configuration| {
            module_roots(configuration).any(|(declared, resolved)| {
                reported == declared.as_path()
                    || physical == resolved
                    || ((reported.starts_with(&declared) || physical.starts_with(&resolved))
                        && is_site_input(reported))
            })
        }) {
            return true;
        }
        if matches!(
            reported.file_name().and_then(std::ffi::OsStr::to_str),
            Some(".gitignore" | ".ignore")
        ) {
            return true;
        }
        // Only a successful check of this revision can exclude unread files: a pending or failed
        // check may have reached dependencies absent from the previous revision's reads.
        let reads_incomplete = !self
            .checked
            .is_some_and(|check| check.revision == self.revision && check.usable_world);
        // The spelling the client reported and the resolved one name one file, and a read may be
        // recorded under either, so both are asked.
        [reported, physical.as_path()].into_iter().any(|path| {
            self.read_paths.contains(path)
                || self.query_read_paths.contains(path)
                || (path.starts_with(root) && (is_site_input(path) || reads_incomplete))
        })
    }

    /// The newest source change becomes the newest revision, superseding every request and check
    /// before it.
    ///
    /// A typing change settles before its check runs; every other change checks at once.
    pub(super) fn sources_changed(&mut self, typing: bool) -> Result<()> {
        self.cancel_pending_work(REVISION_CHANGED_ERROR)?;
        self.query_read_paths.clear();
        self.revision += 1;
        self.checking = BuildCanceller::new();
        let check_mode = self.workspace().check_mode;
        // A site checked on save advances its revision with every edit but checks only for a save:
        // freshness is what that mode trades away, and a check costs the whole site.
        if typing && check_mode == CheckMode::OnSave {
            return self.report_source_check(CheckProgress::NotChecked);
        }
        let now = Instant::now();
        let due = if typing {
            now + CHECK_SETTLE_DELAY
        } else {
            now
        };
        self.check = Some(PendingCheck {
            checked_revision: self.revision,
            queued_at: now,
            due,
            sources: self.source_inputs(self.workspace().root.clone(), self.checking.token()),
        });
        self.report_source_check(CheckProgress::Checking)
    }

    /// Ask the client to report changes below the site root, which is how a file the author never
    /// opened still starts a check.
    pub(super) fn watch_sources(&mut self) -> Result<()> {
        let Phase::Ready(workspace) = &self.phase else {
            return Ok(());
        };
        if !workspace.features.watched_files_dynamic || self.watchers.pending.is_some() {
            return Ok(());
        }
        let relative = workspace.features.watched_files_relative;
        let client_root = crate::uri::ClientRoot::new(&workspace.root);
        let configuration = self.configuration_path(&self.source_root());
        let paths = self.watch_paths(&configuration);
        if self
            .watchers
            .registered
            .as_ref()
            .is_some_and(|watch| watch.paths == paths)
            || self
                .watchers
                .refused
                .as_ref()
                .is_some_and(|(revision, refused)| *revision == self.revision && *refused == paths)
        {
            return Ok(());
        }
        let mut watchers = Vec::with_capacity(paths.len());
        for path in &paths {
            let file = path == &configuration;
            let glob_pattern = if relative {
                GlobPattern::Relative(RelativePattern {
                    base_uri: OneOf::Right(client_root.address(if file {
                        path.parent().expect("a configuration file has a parent")
                    } else {
                        path
                    })?),
                    pattern: if file {
                        escaped_glob_path(Path::new(
                            path.file_name().expect("a configuration file has a name"),
                        ))
                    } else {
                        "**/*".to_owned()
                    },
                })
            } else if file {
                GlobPattern::String(escaped_glob_path(&client_root.spell(path)))
            } else {
                GlobPattern::String(source_glob(&client_root.spell(path)))
            };
            watchers.push(FileSystemWatcher {
                glob_pattern,
                kind: None,
            });
        }
        let id = format!("tola-watched-files-{}", self.next_identifier());
        let request = self.send_request(
            request::RegisterCapability::METHOD,
            RegistrationParams {
                registrations: vec![Registration {
                    id: id.clone(),
                    method: notification::DidChangeWatchedFiles::METHOD.to_owned(),
                    register_options: serde_json::to_value(
                        DidChangeWatchedFilesRegistrationOptions { watchers },
                    )
                    .ok(),
                }],
            },
        )?;
        self.watchers.pending = Some((request, WatchRegistration { id, paths }));
        Ok(())
    }

    /// Keep the old watchers until the client accepts their replacement.
    pub(super) fn watchers_registered(&mut self, response: &Response) -> Result<bool> {
        let Some((_, registration)) = self.watchers.pending.take_if(|(id, _)| *id == response.id)
        else {
            return Ok(false);
        };
        if response.response_result.is_ok() {
            self.watchers.refused = None;
            if let Some(previous) = self.watchers.registered.replace(registration) {
                self.unregister_watchers(previous.id)?;
            }
        } else {
            tracing::warn!("the editor refused Tola's file watchers");
            self.watchers.refused = Some((self.revision, registration.paths));
        }
        // A completed check or query may have changed the paths while the client was answering.
        self.watch_sources()?;
        Ok(true)
    }

    /// Let the client's watchers go, so a restarting server is the one that registers them.
    pub(super) fn unwatch_sources(&mut self) -> Result<()> {
        if let Some(watch) = self.watchers.registered.take() {
            self.unregister_watchers(watch.id)?;
        }
        if let Some((_, watch)) = self.watchers.pending.take() {
            self.unregister_watchers(watch.id)?;
        }
        Ok(())
    }

    fn unregister_watchers(&mut self, id: String) -> Result<()> {
        self.send_request(
            request::UnregisterCapability::METHOD,
            UnregistrationParams {
                unregisterations: vec![Unregistration {
                    id,
                    method: notification::DidChangeWatchedFiles::METHOD.to_owned(),
                }],
            },
        )?;
        Ok(())
    }

    /// The site, package and module directories, plus external configurations and observed inputs.
    fn watch_paths(&self, configuration: &Path) -> Vec<PathBuf> {
        let mut roots = vec![self.source_root()];
        if let Some(locations) = self
            .configuration
            .as_ref()
            .map(|config| config.package_locations())
        {
            let declared = locations
                .declared()
                .iter()
                .filter(|location| location.root().is_dir());
            let explicit = locations
                .data()
                .into_iter()
                .chain(locations.cache())
                .filter(|location| location.source() == PackageLocationSource::Explicit);
            for location in declared.chain(explicit) {
                let root = normalize_existing_prefix(location.root());
                if !roots.iter().any(|watched| root.starts_with(watched)) {
                    roots.push(root);
                }
            }
        }
        if let Some(configuration) = &self.configuration {
            for (declared, physical) in module_roots(configuration) {
                for path in [declared, physical] {
                    let watched = if path.is_dir() {
                        Some(path.as_path())
                    } else {
                        path.parent()
                    };
                    if let Some(watched) = watched
                        && !roots.iter().any(|root| watched.starts_with(root))
                    {
                        roots.push(watched.to_path_buf());
                    }
                }
            }
        }
        if !roots.iter().any(|root| configuration.starts_with(root)) {
            roots.push(configuration.to_path_buf());
        }
        // A parent watcher sees both an input's creation and any nested observed inputs.
        let observed_parents: BTreeSet<_> = self
            .read_paths
            .iter()
            .chain(&self.query_read_paths)
            .filter_map(|input| input.parent())
            .filter(|parent| !roots.iter().any(|root| parent.starts_with(root)))
            .collect();
        for parent in observed_parents {
            if !roots.iter().any(|root| parent.starts_with(root)) {
                roots.push(parent.to_path_buf());
            }
        }
        roots
    }
}

fn source_glob(root: &Path) -> String {
    format!("{}/**/*", escaped_glob_path(root).trim_end_matches('/'))
}

/// Every module root a resolved configuration declares: the spelling the declaration uses, and the
/// physical target it resolves to, which differ when the declaration goes through a link.
fn module_roots(
    configuration: &ResolvedSiteConfig,
) -> impl Iterator<Item = (PathBuf, PathBuf)> + '_ {
    configuration
        .modules()
        .iter()
        .filter_map(|(name, declaration)| {
            let declared = tola_build::inputs::declaration_root(
                configuration.get_root(),
                configuration.vendor().path.as_deref(),
                name,
                &declaration.source,
            )
            .ok()?;
            let physical = normalize_existing_prefix(&declared);
            Some((declared, physical))
        })
}

fn escaped_glob_path(path: &Path) -> String {
    let path = path
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
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{messages, ready_at, ready_workspace};
    use lsp_server::{ErrorCode, Message};
    #[cfg(unix)]
    use serde_json::json;

    #[test]
    fn watcher_globs_escape_root_metacharacters() {
        assert_eq!(
            source_glob(Path::new("/site[a]{b}*?")),
            "/site[[]a[]][{]b[}][*][?]/**/*"
        );
        assert_eq!(source_glob(Path::new("/")), "/**/*");
    }

    #[test]
    fn observed_source_parent_covers_nested_reads() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let site = root.join("site");
        let dependency = root.join("dependency");
        std::fs::create_dir(&site).unwrap();
        let mut connection = ready_at(Vec::new(), site.clone());
        connection.read_paths.extend([
            dependency.join("child/input.typ"),
            dependency.join("source.typ"),
        ]);

        assert_eq!(
            connection.watch_paths(&site.join("tola.toml")),
            [site, dependency]
        );
    }

    #[test]
    fn refused_watchers_leave_the_previous_registration_active() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        std::fs::create_dir(&site).unwrap();
        let mut connection = ready_at(Vec::new(), site);
        ready_workspace(&mut connection)
            .features
            .watched_files_dynamic = true;
        let registration = |connection: &mut Connection<Vec<u8>, _>| {
            let sent = messages(connection);
            let [Message::Request(request)] = sent.as_slice() else {
                panic!("expected one registration, received {sent:?}");
            };
            assert_eq!(request.method, request::RegisterCapability::METHOD);
            request.clone()
        };

        connection.watch_sources().unwrap();
        let original = registration(&mut connection);
        connection.named_configuration = Some(directory.path().join("outside.toml"));
        connection.watch_sources().unwrap();
        assert!(messages(&mut connection).is_empty());
        connection
            .response(Response::new_ok(original.id, ()))
            .unwrap();
        let refused = registration(&mut connection);
        connection
            .response(Response::new_err(
                refused.id,
                ErrorCode::RequestFailed as i32,
                "registration refused".to_owned(),
            ))
            .unwrap();
        connection.watch_sources().unwrap();
        assert!(messages(&mut connection).is_empty());

        connection.sources_changed(false).unwrap();
        connection.watch_sources().unwrap();
        let replacement = registration(&mut connection);
        connection
            .response(Response::new_ok(replacement.id, ()))
            .unwrap();
        let sent = messages(&mut connection);
        let [Message::Request(unregister)] = sent.as_slice() else {
            panic!("expected one unregistration, received {sent:?}");
        };
        assert_eq!(unregister.method, request::UnregisterCapability::METHOD);
        assert_eq!(
            unregister.params["unregisterations"][0]["id"],
            original.params["registrations"][0]["id"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn physical_config_change_starts_check() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        std::fs::create_dir(&site).unwrap();
        let physical = directory.path().join("configuration.toml");
        std::fs::write(&physical, "").unwrap();
        let linked = site.join("tola.toml");
        std::os::unix::fs::symlink(&physical, &linked).unwrap();
        let mut connection = ready_at(Vec::new(), site);
        connection.named_configuration = Some(linked);
        connection.checked = Some(crate::connection::CompletedCheck {
            revision: connection.revision,
            usable_world: true,
        });
        let before = connection.revision;
        let uri = crate::uri::from_file_path(&physical).unwrap();
        let flow = connection
            .notification(Notification::new(
                notification::DidChangeWatchedFiles::METHOD.into(),
                json!({"changes":[{"uri":uri.as_str(),"type":2}]}),
            ))
            .unwrap();
        assert!(matches!(flow, std::ops::ControlFlow::Continue(())));
        assert_eq!(connection.revision, before + 1);
        assert!(connection.check.is_some());
        ready_workspace(&mut connection)
            .features
            .watched_files_dynamic = true;
        connection.watch_sources().unwrap();
        let registered = messages(&mut connection)
            .into_iter()
            .find_map(|message| {
                let lsp_server::Message::Request(request) = message else {
                    return None;
                };
                (request.method == request::RegisterCapability::METHOD).then_some(request.params)
            })
            .unwrap();
        let watchers = registered["registrations"][0]["registerOptions"]["watchers"]
            .as_array()
            .unwrap();
        let physical = normalize_existing_prefix(&physical);
        assert!(
            watchers
                .iter()
                .any(|watcher| { watcher["globPattern"] == physical.to_string_lossy().as_ref() })
        );
    }
}
