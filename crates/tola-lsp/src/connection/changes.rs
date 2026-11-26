//! Watched-file filtering, dynamic watcher registration, and source revision resets.

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
use tola_build::config::SiteConfig;
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
    refused: Option<(u64, Vec<PathBuf>)>,
}

struct WatchRegistration {
    id: String,
    paths: Vec<PathBuf>,
}

fn is_site_input(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|ext| matches!(ext, "typ" | "toml" | "bib" | "csl" | "yml" | "yaml"))
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn watched_change_matters(&self, msg: &Notification) -> bool {
        let Ok(params) =
            serde_json::from_value::<lsp_types::DidChangeWatchedFilesParams>(msg.params.clone())
        else {
            return true;
        };
        if !matches!(self.phase, Phase::Ready(_)) {
            return true;
        }
        let root = self.source_root();
        params.changes.iter().any(|change| match change.typ {
            lsp_types::FileChangeType::CHANGED => crate::uri::to_file_path(change.uri.as_str())
                .map_or(true, |path| self.changed_file_matters(&path, &root)),
            _ => true,
        })
    }

    fn changed_file_matters(&self, reported: &Path, root: &Path) -> bool {
        let physical = normalize_existing_prefix(reported);
        if self.configuration_path(root) == physical || physical == root.join("tola.lock") {
            return true;
        }
        if self.configuration.as_ref().is_some_and(|cfg| {
            module_roots(cfg).any(|(declared, resolved)| {
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
        // Only narrow watched files after a successful check of the current revision.
        let reads_incomplete = !self
            .checked
            .is_some_and(|check| check.revision == self.revision && check.usable_world);
        [reported, physical.as_path()].into_iter().any(|path| {
            self.read_paths.contains(path)
                || self.query_read_paths.contains(path)
                || (path.starts_with(root) && (is_site_input(path) || reads_incomplete))
        })
    }

    pub(super) fn sources_changed(&mut self, typing: bool) -> Result<()> {
        self.cancel_pending_work(REVISION_CHANGED_ERROR)?;
        self.query_read_paths.clear();
        self.revision += 1;
        self.checking = BuildCanceller::new();
        if typing && self.workspace().check_mode == CheckMode::OnSave {
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

    pub(super) fn watch_sources(&mut self) -> Result<()> {
        let Phase::Ready(ws) = &self.phase else {
            return Ok(());
        };
        if !ws.features.watched_files_dynamic || self.watchers.pending.is_some() {
            return Ok(());
        }
        let relative = ws.features.watched_files_relative;
        let client_root = crate::uri::ClientRoot::new(&ws.root);
        let cfg_path = self.configuration_path(&self.source_root());
        let paths = self.watch_paths(&cfg_path);
        if self
            .watchers
            .registered
            .as_ref()
            .is_some_and(|w| w.paths == paths)
            || self
                .watchers
                .refused
                .as_ref()
                .is_some_and(|(rev, refused)| *rev == self.revision && *refused == paths)
        {
            return Ok(());
        }
        let watchers = paths
            .iter()
            .map(|path| build_watcher(path, path == &cfg_path, relative, &client_root))
            .collect::<Result<Vec<_>>>()?;
        let id = format!("tola-watched-files-{}", self.next_identifier());
        let req_id = self.send_request(
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
        self.watchers.pending = Some((req_id, WatchRegistration { id, paths }));
        Ok(())
    }

    pub(super) fn watchers_registered(&mut self, resp: &Response) -> Result<bool> {
        let Some((_, reg)) = self.watchers.pending.take_if(|(id, _)| *id == resp.id) else {
            return Ok(false);
        };
        if resp.response_result.is_ok() {
            self.watchers.refused = None;
            if let Some(prev) = self.watchers.registered.replace(reg) {
                self.unregister_watchers(prev.id)?;
            }
        } else {
            tracing::warn!("the editor refused Tola's file watchers");
            self.watchers.refused = Some((self.revision, reg.paths));
        }
        self.watch_sources()?;
        Ok(true)
    }

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

    fn watch_paths(&self, cfg_path: &Path) -> Vec<PathBuf> {
        let mut roots = vec![self.source_root()];
        let push_root = |roots: &mut Vec<PathBuf>, candidate: &Path| {
            if !roots.iter().any(|root| candidate.starts_with(root)) {
                roots.push(candidate.to_path_buf());
            }
        };
        if let Some(locs) = self.configuration.as_ref().map(|c| c.package_locations()) {
            let declared = locs.declared().iter().filter(|l| l.root().is_dir());
            let explicit = locs
                .data()
                .into_iter()
                .chain(locs.cache())
                .filter(|l| l.source() == PackageLocationSource::Explicit);
            for loc in declared.chain(explicit) {
                push_root(&mut roots, &normalize_existing_prefix(loc.root()));
            }
        }
        if let Some(cfg) = &self.configuration {
            for (declared, physical) in module_roots(cfg) {
                for path in [declared, physical] {
                    let watched = if path.is_dir() {
                        Some(path.as_path())
                    } else {
                        path.parent()
                    };
                    if let Some(watched) = watched {
                        push_root(&mut roots, watched);
                    }
                }
            }
        }
        push_root(&mut roots, cfg_path);
        let observed_parents: BTreeSet<_> = self
            .read_paths
            .iter()
            .chain(&self.query_read_paths)
            .filter_map(|input| input.parent())
            .filter(|parent| !roots.iter().any(|root| parent.starts_with(root)))
            .collect();
        for parent in observed_parents {
            push_root(&mut roots, parent);
        }
        roots
    }
}

fn build_watcher(
    path: &Path,
    is_file: bool,
    relative: bool,
    client_root: &crate::uri::ClientRoot,
) -> Result<FileSystemWatcher> {
    let glob_pattern = if relative {
        GlobPattern::Relative(RelativePattern {
            base_uri: OneOf::Right(client_root.address(if is_file {
                path.parent().expect("a configuration file has a parent")
            } else {
                path
            })?),
            pattern: if is_file {
                escaped_glob_path(Path::new(
                    path.file_name().expect("a configuration file has a name"),
                ))
            } else {
                "**/*".to_owned()
            },
        })
    } else if is_file {
        GlobPattern::String(escaped_glob_path(&client_root.spell(path)))
    } else {
        GlobPattern::String(source_glob(&client_root.spell(path)))
    };
    Ok(FileSystemWatcher {
        glob_pattern,
        kind: None,
    })
}

fn source_glob(root: &Path) -> String {
    format!("{}/**/*", escaped_glob_path(root).trim_end_matches('/'))
}

pub(super) fn module_roots(cfg: &SiteConfig) -> impl Iterator<Item = (PathBuf, PathBuf)> + '_ {
    cfg.modules().iter().filter_map(|(name, decl)| {
        let declared = tola_build::inputs::declaration_root(
            cfg.root(),
            cfg.vendor().path.as_deref(),
            name,
            &decl.source,
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
    for ch in path.chars() {
        if matches!(ch, '[' | ']' | '{' | '}' | '*' | '?') {
            pattern.push('[');
            pattern.push(ch);
            pattern.push(']');
        } else {
            pattern.push(ch);
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
