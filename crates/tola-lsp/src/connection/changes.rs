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
        hover_query, open_document, ready, ready_at, rejection, response,
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
}
