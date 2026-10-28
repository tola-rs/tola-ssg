//! The one command this server runs — the preview lens's — and where the site is served.

use std::io::Write;

use anyhow::Result;
use lsp_server::{ErrorCode, RequestId, Response};
use lsp_types::ShowDocumentParams;
use lsp_types::request::{self, Request as LspRequest};
use tola_diagnostic::Diagnostic;

use crate::protocol::{self, PreviewReply, PreviewRequest, RouteParams};

use super::{Connection, Phase};

/// The reply to a command this server does not run, which is the one thing a client can get wrong
/// in a well-formed `workspace/executeCommand`.
const UNKNOWN_COMMAND: &str = "Tola runs no such command";

/// The reply to a preview command whose arguments name no document, so there is no route to open.
const PREVIEW_NEEDS_DOCUMENT: &str = "the preview command named no document";

/// Why the preview command could not be answered.
#[derive(Debug, thiserror::Error)]
enum CommandError {
    #[error("the page address is not a URI")]
    PageAddress,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// Run one command the client asked for.
    ///
    /// The one command this server serves is the preview lens's: it opens the page a lens names,
    /// through the client when the client can open documents and as an address the author opens
    /// otherwise. A lens that named no route offered a choice of pages, and the answer is that
    /// choice. The server advertises no `executeCommandProvider`: the command belongs to the
    /// client's own extension, and a client that registers every command a server offers would
    /// collide with the one it already holds.
    pub(super) fn command(
        &mut self,
        id: RequestId,
        parameters: lsp_types::ExecuteCommandParams,
    ) -> Result<()> {
        if parameters.command != protocol::PREVIEW_COMMAND {
            return self.reject(id, ErrorCode::MethodNotFound, UNKNOWN_COMMAND);
        }
        let Some(request) = PreviewRequest::decode(&parameters.arguments) else {
            return self.reject(id, ErrorCode::InvalidParams, PREVIEW_NEEDS_DOCUMENT);
        };
        let Some(route) = request.route else {
            return self.route(id, RouteParams { uri: request.uri });
        };
        let url = self.preview_url(&route);
        let showing = match &url {
            Some(url) => self.show_document(url)?,
            None => false,
        };
        let message = match (&url, showing) {
            (Some(url), true) => format!("Tola asked the editor to open {url}."),
            (Some(url), false) => format!("this page is served at {url}"),
            (None, _) => "Tola does not know where this site is served; run `tola dev` to serve it"
                .to_owned(),
        };
        self.reply(Response::new_ok(
            id,
            PreviewReply {
                url,
                showing,
                message,
            },
        ))
    }

    /// Routes already hold the configured mount; only the serving origin is added here.
    pub(super) fn preview_url(&self, route: &str) -> Option<String> {
        let origin = self.preview_origin()?;
        Some(format!("{}{route}", origin.trim_end_matches('/')))
    }

    /// Where this site is served, as `http://host:port`.
    ///
    /// A client that named an origin outranks a command line, which outranks the address the site's
    /// own state holds: an author states a location once, and the running server is the only source
    /// that knows a port it had to fall back to.
    pub(super) fn preview_origin(&self) -> Option<String> {
        self.named_origin
            .clone()
            .or_else(|| self.default_origin.clone())
            .or_else(|| self.dev_server_address())
    }

    /// The address the site's own `tola dev` wrote, when one is serving.
    ///
    /// The address names the site's own root, which a configuration may place outside the folder
    /// the client named.
    fn dev_server_address(&self) -> Option<String> {
        let Phase::Ready(workspace) = &self.phase else {
            return None;
        };
        let root = self
            .configuration
            .as_ref()
            .map_or(workspace.root.as_path(), |configuration| {
                configuration.get_root()
            });
        tola_build::filesystem::DevServerAddress::read(root)
    }

    /// Ask the client to open one address, which only a client that declared it can do.
    fn show_document(&mut self, url: &str) -> std::result::Result<bool, CommandError> {
        let Phase::Ready(workspace) = &self.phase else {
            return Ok(false);
        };
        if !workspace.features.show_document {
            return Ok(false);
        }
        let uri: lsp_types::Uri = url.parse().map_err(|_| CommandError::PageAddress)?;
        self.send_request(
            request::ShowDocument::METHOD,
            ShowDocumentParams {
                uri,
                external: Some(true),
                take_focus: Some(true),
                selection: None,
            },
        )?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use crate::connection::tests::ready_at;

    /// A running server is the only source that knows the port it fell back to, so the address it
    /// wrote names where the site is served.
    #[test]
    fn dev_server_address_locates_the_site() {
        let directory = tempfile::tempdir().unwrap();
        let connection = ready_at(Vec::new(), directory.path().to_path_buf());
        assert_eq!(connection.preview_origin(), None);
        let written = tola_build::filesystem::DevServerAddress::write(
            directory.path(),
            "http://localhost:4321",
        )
        .unwrap();
        assert_eq!(
            connection.preview_origin().as_deref(),
            Some("http://localhost:4321")
        );
        drop(written);
    }

    #[test]
    fn dev_address_follows_site_root() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        std::fs::create_dir_all(site.join("content")).unwrap();
        let config = crate::compiler::tests::site_configuration(&site, "");
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        connection.configuration = Some(config);

        let _written =
            tola_build::filesystem::DevServerAddress::write(&site, "http://localhost:4321")
                .unwrap();
        assert_eq!(
            connection.preview_origin().as_deref(),
            Some("http://localhost:4321")
        );
    }

    #[test]
    fn preview_mount_is_applied_once() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        let config = crate::compiler::tests::site_configuration(
            directory.path(),
            "[site]\nbase-path = \"/docs/\"\n",
        );
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        connection.configuration = Some(config);
        connection.named_origin = Some("http://localhost:9000".to_owned());
        assert_eq!(
            connection.preview_url("/docs/post/").as_deref(),
            Some("http://localhost:9000/docs/post/")
        );
    }
}
