//! The handshake and the client settings that shape this connection: the phase it serves, the
//! workspace it was initialized for, and what a settings change re-reads.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_server::{ErrorCode, Notification, RequestId, Response};
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    ConfigurationItem, ConfigurationParams, InitializeParams, WorkDoneProgress,
    WorkDoneProgressBegin,
};
use tola_build::diagnostic::Diagnostic;

use crate::capabilities::ClientFeatures;
use crate::diagnostic::ClientDiagnostics;
use crate::protocol::{self, InitializationOptions, Settings};
use crate::sources::OpenSources;

use super::progress::PROGRESS_TITLE;
use super::routing::InvalidNotification;
use super::{Connection, Phase, Workspace};

/// The settings section a client answers `workspace/configuration` for, which is the only section
/// Tola reads.
const SETTINGS_SECTION: &str = "tola";

impl Phase {
    pub(super) fn request_error(&self, method: &str) -> Option<(ErrorCode, &'static str)> {
        use ErrorCode::{InvalidRequest, ServerNotInitialized};
        match self {
            Self::Uninitialized if method == request::Initialize::METHOD => None,
            _ if method == request::Initialize::METHOD => {
                Some((InvalidRequest, "language server is already initialized"))
            }
            Self::Uninitialized => {
                Some((ServerNotInitialized, "language server is not initialized"))
            }
            Self::Shutdown => Some((InvalidRequest, "language server has shut down")),
            Self::Initializing(_) if method != request::Shutdown::METHOD => Some((
                ServerNotInitialized,
                "language server is awaiting initialized",
            )),
            _ => None,
        }
    }
}

impl Workspace {
    fn initialize(parameters: InitializeParams) -> Result<Self> {
        let root = initialization_root(&parameters)?;
        let features = ClientFeatures::new(&parameters.capabilities);
        let options: InitializationOptions = parameters
            .initialization_options
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let formatter = options.formatter;
        let check_mode = options.check_mode;
        let preview_origin = options.preview_origin;
        let line_folding_only = parameters
            .capabilities
            .text_document
            .as_ref()
            .and_then(|document| document.folding_range.as_ref())
            .and_then(|folding| folding.line_folding_only)
            .unwrap_or(false);
        let routes_as_hints = super::routes_as_hints(&parameters.capabilities);
        let package_sources = options.package_source_directory.map(|directory| {
            Arc::new(if directory.is_absolute() {
                directory
            } else {
                root.join(directory)
            })
        });
        Ok(Self {
            diagnostics: ClientDiagnostics::new(root.clone()),
            root,
            package_sources,
            formatter,
            check_mode,
            source_check_status: options.source_check_status,
            preview_origin,
            line_folding_only,
            routes_as_hints,
            features,
        })
    }
}

#[expect(
    deprecated,
    reason = "LSP clients may still select a workspace through rootUri or rootPath"
)]
fn initialization_root(parameters: &InitializeParams) -> Result<PathBuf> {
    let folders = parameters.workspace_folders.as_deref().unwrap_or_default();
    anyhow::ensure!(
        folders.len() <= 1,
        "Tola serves one workspace folder at a time; open this site in a window of its own"
    );
    if let Some(uri) = parameters
        .root_uri
        .as_ref()
        .or_else(|| folders.first().map(|folder| &folder.uri))
    {
        // The client's own spelling of the root is kept: diagnostics are published back under the
        // identity the editor opened, while paths are normalized wherever Tola compares them.
        return crate::uri::to_file_path(uri.as_str())
            .context("the workspace root must be a local file URI");
    }
    if let Some(root) = &parameters.root_path {
        return std::path::absolute(root).context("resolve workspace root");
    }
    std::env::current_dir().context("resolve workspace directory")
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// Answer the client's `initialize`: the workspace this connection serves, and everything it
    /// negotiated from the client's capabilities.
    pub(super) fn initialize(&mut self, id: RequestId, parameters: InitializeParams) -> Result<()> {
        // The launch configuration names where the site is served, and it stands in for the site's
        // own generated state until a settings change names an origin of its own.
        match Workspace::initialize(parameters) {
            Ok(workspace) => {
                self.open = OpenSources::new(&workspace.root);
                self.default_origin = workspace.preview_origin.clone();
                let reply = protocol::initialize_result(&workspace.features);
                self.phase = Phase::Initializing(workspace);
                self.reply(Response::new_ok(id, reply))
            }
            Err(error) => self.reject(id, ErrorCode::InvalidParams, &error.to_string()),
        }
    }

    /// Complete the handshake, whether or not the client's `initialized` body was readable.
    ///
    /// `initialized` has nothing Tola reads, so the handshake completes here instead of through
    /// the document handler: Tola must be ready for the first document even when a client sends this
    /// notification with an unexpected body.
    pub(super) fn initialized(&mut self) -> Result<()> {
        match std::mem::replace(&mut self.phase, Phase::Uninitialized) {
            Phase::Initializing(workspace) => {
                let watched = workspace.features.watched_files_dynamic;
                self.phase = Phase::Ready(workspace);
                self.sources_changed(false)?;
                if watched {
                    self.watch_sources()?;
                }
            }
            // `initialized` before `initialize`, or a second one, changes nothing.
            other => self.phase = other,
        }
        Ok(())
    }

    /// Re-read this client's settings, from the notification itself or from the client.
    ///
    /// A notification that has settings has the whole section, as a client states it in its
    /// own configuration file; one that has none asks the server to read it, which only a
    /// client answering `workspace/configuration` can. One section is read, and a section a client
    /// does not hold leaves every setting as it stands.
    pub(super) fn configured(&mut self, message: Notification) -> Result<()> {
        let parameters = match message.extract::<lsp_types::DidChangeConfigurationParams>(
            notification::DidChangeConfiguration::METHOD,
        ) {
            Ok(parameters) => parameters,
            Err(_) => return self.log_invalid_notification(InvalidNotification::Configuration),
        };
        let settings = Settings::read(&parameters.settings);
        if !settings.is_empty() {
            self.settings_pull = None;
            self.apply_settings(settings);
            return Ok(());
        }
        let Phase::Ready(workspace) = &self.phase else {
            return Ok(());
        };
        if !workspace.features.configuration {
            return Ok(());
        }
        let id = self.send_request(
            request::WorkspaceConfiguration::METHOD,
            ConfigurationParams {
                items: vec![ConfigurationItem {
                    scope_uri: None,
                    section: Some(SETTINGS_SECTION.to_owned()),
                }],
            },
        )?;
        self.settings_pull = Some(id);
        Ok(())
    }

    /// Apply the settings one shape supplied, leaving every setting it did not name as it stands.
    fn apply_settings(&mut self, settings: Settings) {
        let Phase::Ready(workspace) = &mut self.phase else {
            return;
        };
        if let Some(width) = settings.formatter.print_width {
            workspace.formatter.print_width = width;
        }
        if let Some(wrap) = settings.formatter.prose_wrap {
            workspace.formatter.prose_wrap = wrap;
        }
        if let Some(mode) = settings.check_mode {
            workspace.check_mode = mode;
        }
        if let Some(origin) = settings.preview_origin {
            self.named_origin = Some(origin);
        }
    }

    /// Apply what the client answered to a request this connection sent.
    pub(super) fn response(&mut self, response: lsp_server::Response) -> Result<()> {
        if self
            .progress_create
            .as_ref()
            .is_some_and(|creation| creation.id == response.id)
        {
            let creation = self
                .progress_create
                .take()
                .expect("matching progress creation");
            // A client that refused one report is never sent another: nothing Tola does needs a
            // report, and repeating a refused request is noise.
            self.progress_agreed = response.response_result.is_ok();
            if !self.progress_agreed {
                self.progress = None;
            } else if let Some(progress) = self.progress.as_mut()
                && progress.token == creation.token
            {
                progress.opened = true;
                self.report_progress(
                    creation.token,
                    WorkDoneProgress::Begin(WorkDoneProgressBegin {
                        title: PROGRESS_TITLE.to_owned(),
                        cancellable: Some(true),
                        message: None,
                        percentage: None,
                    }),
                )?;
            }
            return Ok(());
        }
        if self.settings_pull.as_ref() == Some(&response.id) {
            self.settings_pull = None;
            if let Ok(sections) = response.response_result
                && let Some(section) = sections.as_array().and_then(|sections| sections.first())
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
    use lsp_server::{Message, Request};
    use lsp_types::Uri;
    use serde_json::json;
    use std::time::Instant;

    #[test]
    fn initialization_refuses_bad_roots() {
        let parameters = |value| serde_json::from_value::<InitializeParams>(value).unwrap();
        assert!(
            initialization_root(&parameters(
                json!({"capabilities":{},"rootUri":"https://example.test/site"})
            ))
            .is_err()
        );
        assert!(initialization_root(&parameters(json!({"capabilities":{},"rootUri":"file:///site", "workspaceFolders":[{"uri":"file:///site","name":"site"},{"uri":"file:///other","name":"other"}]}))).is_err());
    }

    /// A client whose `initialized` body Tola cannot read must still reach the ready phase:
    /// otherwise every later notification is dropped and no source is ever tracked.
    #[test]
    fn unreadable_handshake_still_readies() {
        let mut connection = uninitialized();
        initialize(&mut connection);
        let _ = connection
            .notification(Notification::new(
                notification::Initialized::METHOD.into(),
                serde_json::Value::Null,
            ))
            .unwrap();
        assert!(matches!(connection.phase, Phase::Ready(_)));
        assert!(matches!(
            connection.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
    }

    #[test]
    fn repeated_initialized_changes_nothing() {
        let mut connection = uninitialized();
        let _ = connection
            .notification(Notification::new(
                notification::Initialized::METHOD.into(),
                json!({}),
            ))
            .unwrap();
        assert!(matches!(connection.phase, Phase::Uninitialized));
        initialize(&mut connection);
        let _ = connection
            .notification(Notification::new(
                notification::Initialized::METHOD.into(),
                json!({}),
            ))
            .unwrap();
        let _ = connection
            .notification(Notification::new(
                notification::Initialized::METHOD.into(),
                json!({}),
            ))
            .unwrap();
        assert!(matches!(connection.phase, Phase::Ready(_)));
    }

    /// A document notification that arrives between the `initialize` reply and `initialized` must
    /// not crash the connection or leave a buffer the handshake never accepted.
    #[test]
    fn notifications_before_initialized_are_dropped() {
        let mut connection = uninitialized();
        initialize(&mut connection);
        let uri: Uri = "file:///site/page.typ".parse().unwrap();
        open_document(&mut connection, &uri, 1, "#let draft = 1");
        assert!(matches!(connection.phase, Phase::Initializing(_)));
        assert!(connection.open.source(&uri).is_none());
        assert!(connection.next_job(Instant::now()).is_none());
        let _ = connection
            .notification(Notification::new(
                notification::Initialized::METHOD.into(),
                json!({}),
            ))
            .unwrap();
        assert!(matches!(connection.phase, Phase::Ready(_)));
        assert!(matches!(
            connection.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
    }

    /// A launch configuration is the command line that named a preview origin: the client states
    /// it in `initializationOptions`, which is where every other setting of the first session
    /// arrives.
    #[test]
    fn initialization_options_name_the_preview_origin() {
        let mut connection = uninitialized();
        let root = std::env::current_dir().unwrap();
        connection
            .request(Request::new(
                1.into(),
                request::Initialize::METHOD.into(),
                json!({
                    "workspaceFolders":[{"uri": crate::uri::from_file_path(&root).unwrap(), "name":"site"}],
                    "capabilities":{},
                    "initializationOptions":{"previewOrigin":"http://localhost:9999"},
                }),
            ))
            .unwrap();
        assert!(matches!(connection.phase, Phase::Initializing(_)));
        assert_eq!(
            connection.preview_origin().as_deref(),
            Some("http://localhost:9999")
        );
    }

    /// A settings notification that has no settings asks the client for the one section Tola
    /// reads, and an answer with no section leaves every setting as it stands.
    #[test]
    fn inline_settings_supersede_pulls() {
        let mut connection = ready(Vec::new());
        ready_workspace(&mut connection).features.configuration = true;
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeConfiguration::METHOD.into(),
                json!({"settings": null}),
            ))
            .unwrap();
        let Message::Request(pulled) = messages(&mut connection).remove(0) else {
            panic!("settings request");
        };
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeConfiguration::METHOD.into(),
                json!({"settings": {"formatter": {"printWidth": 80}}}),
            ))
            .unwrap();
        connection
            .response(Response::new_ok(
                pulled.id,
                json!([{
                    "formatter": {"printWidth": 120}
                }]),
            ))
            .unwrap();
        assert_eq!(connection.workspace().formatter.print_width, 80);
    }

    #[test]
    fn settings_pull_reads_only_the_tola_section() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.features.configuration = true;
        workspace.formatter.print_width = 93;
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeConfiguration::METHOD.into(),
                json!({"settings": null}),
            ))
            .unwrap();
        let written = messages(&mut connection);
        let Some(Message::Request(pulled)) = written.first() else {
            panic!("a settings change pulls the section: {written:?}");
        };
        assert_eq!(pulled.method, request::WorkspaceConfiguration::METHOD);
        let parameters: lsp_types::ConfigurationParams =
            serde_json::from_value(pulled.params.clone()).unwrap();
        assert_eq!(parameters.items.len(), 1);
        assert_eq!(
            parameters.items[0].section.as_deref(),
            Some(SETTINGS_SECTION)
        );
        connection
            .response(Response::new_ok(pulled.id.clone(), json!([null])))
            .unwrap();
        let workspace = connection.workspace();
        assert_eq!(workspace.formatter.print_width, 93);
    }
}
