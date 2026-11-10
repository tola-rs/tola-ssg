//! Typst Universe downloads over the HTTP client the rest of tola uses.
//!
//! `typst-kit` ships a downloader built on `ureq` and `native-tls`, which links a second TLS
//! stack (OpenSSL) into every release binary. The `network` feature installs this one instead so
//! Typst packages travel the same `rustls` client as the rest of the workspace.

use std::any::Any;
use std::io::{self, Read};
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::USER_AGENT;
use typst_kit::downloader::Downloader;

/// Seconds a Typst Universe connection may take to establish before it fails.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Downloads Typst Universe packages and mirrors over the workspace TLS stack.
#[derive(Debug, Clone)]
pub struct HttpDownloader {
    user_agent: String,
}

impl HttpDownloader {
    /// Create a downloader that identifies itself as `user_agent`.
    pub fn new(user_agent: impl Into<String>) -> Self {
        Self {
            user_agent: user_agent.into(),
        }
    }
}

/// The process-wide blocking client. One client shares one connection pool and one runtime
/// thread, however many package stores a build creates.
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
        let response = client()?
            .get(url)
            .header(USER_AGENT, &self.user_agent)
            .send()
            .map_err(io::Error::other)?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{url} is not available"),
            ));
        }
        if !status.is_success() {
            return Err(io::Error::other(format!("{url} answered {status}")));
        }
        Ok((
            response.content_length().map(|size| size as usize),
            Box::new(response),
        ))
    }
}
