//! Watched files and source revisions: what a change to a file costs this connection, and the
//! watchers that make a file the author never opened start a check.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use lsp_server::Notification;
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, OneOf, Registration,
    RegistrationParams, RelativePattern, Unregistration, UnregistrationParams,
};
use tola_build::cancellation::BuildCanceller;
use tola_build::config::ResolvedSiteConfig;
use tola_build::filesystem::normalize_existing_prefix;
use tola_diagnostic::Diagnostic;

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
        if !workspace.features.watched_files_dynamic {
            return Ok(());
        }
        let relative = workspace.features.watched_files_relative;
        let client_root = crate::uri::ClientRoot::new(&workspace.root);
        let configuration = self.configuration_path(&self.source_root());
        let paths = self.watch_paths(&configuration);
        if self.watched_paths == paths {
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
        self.watched_paths = paths;
        Ok(())
    }

    /// Let the client's watchers go, so a restarting server is the one that registers them.
    pub(super) fn unwatch_sources(&mut self) -> Result<()> {
        if !self.watched_paths.is_empty() {
            self.send_request(
                request::UnregisterCapability::METHOD,
                UnregistrationParams {
                    unregisterations: vec![Unregistration {
                        id: WATCHED_FILES.to_owned(),
                        method: notification::DidChangeWatchedFiles::METHOD.to_owned(),
                    }],
                },
            )?;
            self.watched_paths.clear();
        }
        Ok(())
    }

    /// The site and package directories, plus the configuration file when its physical target
    /// falls outside those directories.
    fn watch_paths(&self, configuration: &Path) -> Vec<PathBuf> {
        let mut roots = vec![self.source_root()];
        if let Some(locations) = self
            .configuration
            .as_ref()
            .map(|config| config.package_locations())
        {
            for location in locations.declared() {
                let root = normalize_existing_prefix(location.root());
                if root.is_dir() && !roots.iter().any(|watched| root.starts_with(watched)) {
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
        for input in &self.query_read_paths {
            // Watch the parent so creating a previously missing input also starts a revision.
            let Some(parent) = input.parent() else {
                continue;
            };
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
            let declared = tola_build::modules::declared_root(
                configuration.get_root(),
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
    use crate::connection::tests::{
        answer_analysis, hover_query, last_position, messages, open_document, ready, ready_at,
        ready_workspace, rejection, response,
    };
    use lsp_server::{ErrorCode, Request, RequestId};
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use tola_typst::typst::syntax::Source;
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
        let position = last_position(text, "count");
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
        answer_analysis(&mut connection, pending);
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
