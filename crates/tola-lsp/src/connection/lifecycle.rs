//! Connection initialization handshake and workspace/client settings updates.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use lsp_server::{ErrorCode, Notification, RequestId, Response};
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{ConfigurationItem, ConfigurationParams, InitializeParams};
use tola_diagnostic::Diagnostic;

use crate::capabilities::ClientFeatures;
use crate::diagnostic::ClientDiagnostics;
use crate::protocol::{self, InitializationOptions, Settings};
use crate::sources::OpenSources;

use super::routing::InvalidNotification;
use super::{Connection, Phase, Workspace};

const SETTINGS_SECTION: &str = "tola";

impl Phase {
    pub(super) fn request_error(&self, method: &str) -> Option<(ErrorCode, &'static str)> {
        use ErrorCode::{InvalidRequest, ServerNotInitialized};
        if method == request::Initialize::METHOD {
            return (!matches!(self, Self::Uninitialized))
                .then_some((InvalidRequest, "language server is already initialized"));
        }
        match self {
            Self::Uninitialized => {
                Some((ServerNotInitialized, "language server is not initialized"))
            }
            Self::Shutdown => Some((InvalidRequest, "language server has shut down")),
            Self::Initializing(_) if method != request::Shutdown::METHOD => Some((
                ServerNotInitialized,
                "language server is awaiting initialized",
            )),
            Self::Initializing(_) | Self::Ready(_) => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum WorkspaceError {
    #[error("Tola serves one workspace folder at a time; open this site in a window of its own")]
    SeveralFolders,
    #[error("the workspace root must be a local file URI")]
    RootNotLocal(#[source] crate::uri::UriError),
    #[error("the workspace directory could not be read")]
    UnresolvableDirectory(#[source] std::io::Error),
    #[error("the initialization options Tola reads are not valid")]
    Options(#[from] serde_json::Error),
}

impl Workspace {
    fn initialize(
        params: InitializeParams,
    ) -> Result<(Self, Option<Arc<PathBuf>>), WorkspaceError> {
        let root = initialization_root(&params)?;
        let features = ClientFeatures::new(&params.capabilities);
        let opts: InitializationOptions = params
            .initialization_options
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let line_folding_only = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|doc| doc.folding_range.as_ref())
            .and_then(|f| f.line_folding_only)
            .unwrap_or(false);
        let pkg_sources = opts.package_source_directory.map(|dir| {
            Arc::new(if dir.is_absolute() {
                dir
            } else {
                root.join(dir)
            })
        });
        Ok((
            Self {
                diagnostics: ClientDiagnostics::new(root.clone()),
                root,
                check_mode: opts.check_mode,
                source_check_status: opts.source_check_status,
                preview_origin: opts.preview_origin,
                line_folding_only,
                routes_as_hints: super::routes_as_hints(&params.capabilities),
                features,
            },
            pkg_sources,
        ))
    }
}

fn initialization_root(params: &InitializeParams) -> Result<PathBuf, WorkspaceError> {
    let folders = params.workspace_folders.as_deref().unwrap_or_default();
    if folders.len() > 1 {
        return Err(WorkspaceError::SeveralFolders);
    }
    if let Some(folder) = folders.first() {
        // Keep the client's spelling of the root for published URIs.
        return crate::uri::to_file_path(folder.uri.as_str()).map_err(WorkspaceError::RootNotLocal);
    }
    std::env::current_dir().map_err(WorkspaceError::UnresolvableDirectory)
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn initialize(&mut self, id: RequestId, params: InitializeParams) -> Result<()> {
        match Workspace::initialize(params) {
            Ok((ws, pkg_sources)) => {
                self.default_origin = ws.preview_origin.clone();
                let reply = protocol::initialize_result(&ws.features);
                self.phase = Phase::Initializing(ws);
                self.open = OpenSources::new(&self.source_root()).with_package_sources(pkg_sources);
                self.reply(Response::new_ok(id, reply))
            }
            Err(err) => {
                tracing::error!(error = ?err, "the workspace could not be initialized");
                self.reject(id, ErrorCode::InvalidParams, &err.to_string())
            }
        }
    }

    pub(super) fn initialized(&mut self) -> Result<()> {
        match std::mem::replace(&mut self.phase, Phase::Uninitialized) {
            Phase::Initializing(ws) => {
                let watched = ws.features.watched_files_dynamic;
                self.phase = Phase::Ready(ws);
                self.sources_changed(false)?;
                if watched {
                    self.watch_sources()?;
                }
            }
            other => self.phase = other,
        }
        Ok(())
    }

    pub(super) fn configured(&mut self, msg: Notification) -> Result<()> {
        let params = match msg.extract::<lsp_types::DidChangeConfigurationParams>(
            notification::DidChangeConfiguration::METHOD,
        ) {
            Ok(params) => params,
            Err(_) => return self.log_invalid_notification(InvalidNotification::Configuration),
        };
        let section = params
            .settings
            .get(SETTINGS_SECTION)
            .unwrap_or(&params.settings);
        let settings = Settings::read(section);
        if !settings.is_empty() {
            self.settings_pull = None;
            self.apply_settings(settings);
            return Ok(());
        }
        let Phase::Ready(ws) = &self.phase else {
            return Ok(());
        };
        if !ws.features.configuration {
            return Ok(());
        }
        let id = self.send_request(
            request::WorkspaceConfiguration::METHOD,
            ConfigurationParams {
                items: vec![ConfigurationItem {
                    scope_uri: Some(crate::uri::from_file_path(&ws.root)?),
                    section: Some(SETTINGS_SECTION.to_owned()),
                }],
            },
        )?;
        self.settings_pull = Some(id);
        Ok(())
    }

    fn apply_settings(&mut self, settings: Settings) {
        let Phase::Ready(ws) = &mut self.phase else {
            return;
        };
        if let Some(mode) = settings.check_mode {
            ws.check_mode = mode;
        }
        if let Some(origin) = settings.preview_origin {
            self.named_origin = Some(origin);
        }
    }

    pub(super) fn response(&mut self, resp: Response) -> Result<()> {
        if self.watchers_registered(&resp)? {
            return Ok(());
        }
        if self.semantic_tokens_refreshed(&resp)? {
            return Ok(());
        }
        if self.progress_created(&resp)? {
            return Ok(());
        }
        if self.settings_pull.as_ref() == Some(&resp.id) {
            self.settings_pull = None;
            if let Ok(sections) = resp.response_result
                && let Some(section) = sections.as_array().and_then(|s| s.first())
            {
                self.apply_settings(Settings::read(section));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::SourceJob;
    use crate::connection::tests::{
        initialize, messages, open_document, ready, ready_workspace, uninitialized,
    };
    use crate::protocol::CheckMode;
    use lsp_server::Message;
    use lsp_types::Uri;
    use serde_json::json;
    use std::time::Instant;

    #[test]
    fn initialization_rejects_invalid_roots() {
        let params = |val| serde_json::from_value::<InitializeParams>(val).unwrap();
        assert!(
            initialization_root(&params(json!({
                "capabilities":{},
                "workspaceFolders":[{"uri":"https://example.test/site","name":"site"}],
            })))
            .is_err()
        );
        assert!(
            initialization_root(&params(json!({
                "capabilities":{},
                "workspaceFolders":[
                    {"uri":"file:///site","name":"site"},
                    {"uri":"file:///other","name":"other"},
                ],
            })))
            .is_err()
        );
    }

    #[test]
    fn notifications_before_initialized_dropped() {
        let mut conn = uninitialized();
        initialize(&mut conn);
        let uri: Uri = "file:///site/page.typ".parse().unwrap();
        open_document(&mut conn, &uri, 1, "#let draft = 1");
        assert!(matches!(conn.phase, Phase::Initializing(_)));
        assert!(conn.open.source(&uri).is_none());
        assert!(conn.next_job(Instant::now()).is_none());
        let _ = conn
            .notification(Notification::new(
                notification::Initialized::METHOD.into(),
                json!({}),
            ))
            .unwrap();
        assert!(matches!(conn.phase, Phase::Ready(_)));
        assert!(matches!(
            conn.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
    }

    #[test]
    fn inline_settings_supersede_pulls() {
        for settings in [
            json!({"checkMode":"onSave"}),
            json!({"tola":{"checkMode":"onSave"}}),
        ] {
            let mut conn = ready(Vec::new());
            ready_workspace(&mut conn).features.configuration = true;
            let _ = conn
                .notification(Notification::new(
                    notification::DidChangeConfiguration::METHOD.into(),
                    json!({"settings": null}),
                ))
                .unwrap();
            let Message::Request(pulled) = messages(&mut conn).remove(0) else {
                panic!("settings request");
            };
            let _ = conn
                .notification(Notification::new(
                    notification::DidChangeConfiguration::METHOD.into(),
                    json!({"settings": settings}),
                ))
                .unwrap();
            conn.response(Response::new_ok(
                pulled.id,
                json!([{"checkMode": "onType"}]),
            ))
            .unwrap();
            assert_eq!(conn.workspace().check_mode, CheckMode::OnSave);
        }
    }

    #[test]
    fn settings_pull_names_workspace_scope() {
        let mut conn = ready(Vec::new());
        ready_workspace(&mut conn).features.configuration = true;
        conn.configured(Notification::new(
            notification::DidChangeConfiguration::METHOD.into(),
            json!({"settings": null}),
        ))
        .unwrap();
        let Message::Request(pulled) = messages(&mut conn).remove(0) else {
            panic!("settings request");
        };
        let root = crate::uri::from_file_path(&conn.workspace().root).unwrap();
        assert_eq!(pulled.params["items"][0]["scopeUri"], root.as_str());
    }
}
