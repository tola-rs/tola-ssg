//! Typst Universe downloads over the HTTP client the rest of tola uses.
//!
//! `typst-kit` ships a downloader built on `ureq` and `native-tls`, which links a second TLS
//! stack (OpenSSL) into every release binary. The `network` feature installs this one instead so
//! Typst packages travel the same `rustls` client as the rest of the workspace.

use std::any::Any;
use std::io::{self, Cursor, Read};
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use reqwest::header::USER_AGENT;
use typst_kit::downloader::Downloader;

/// Seconds a Typst Universe connection may take to establish before it fails.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Downloads Typst Universe packages and mirrors over the workspace TLS stack.
pub(crate) struct HttpDownloader {
    user_agent: String,
    cancellation: crate::BundleCancellation,
}

impl HttpDownloader {
    /// Create a downloader that identifies itself as `user_agent`.
    pub(crate) fn new(
        user_agent: impl Into<String>,
        cancellation: crate::BundleCancellation,
    ) -> Self {
        Self {
            user_agent: user_agent.into(),
            cancellation,
        }
    }
}

/// One connection pool for all Typst package stores.
fn client() -> io::Result<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(io::Error::other)?;
    Ok(CLIENT.get_or_init(|| client))
}

impl Downloader for HttpDownloader {
    fn stream(&self, _: &dyn Any, url: &str) -> io::Result<(Option<usize>, Box<dyn Read>)> {
        let bytes = self.download(&(), url)?;
        Ok((Some(bytes.len()), Box::new(Cursor::new(bytes))))
    }

    fn download(&self, _: &dyn Any, url: &str) -> io::Result<Vec<u8>> {
        let request = client()?.get(url).header(USER_AGENT, &self.user_agent);
        tola_input::http::wait(
            self.cancellation.flag(),
            async move { download(request).await },
        )
        .map_err(io::Error::other)?
    }
}

async fn download(request: reqwest::RequestBuilder) -> io::Result<Vec<u8>> {
    let response = request.send().await.map_err(io::Error::other)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is not available", response.url()),
        ));
    }
    if !status.is_success() {
        return Err(io::Error::other(format!(
            "{} answered {status}",
            response.url()
        )));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(io::Error::other)
}
