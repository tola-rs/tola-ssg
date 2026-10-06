//! Editor access to published HTML routes and the saved-input publication fence.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;
use tola_build::config::ResolvedSiteConfig;
use tola_build::site::SiteRevision;
use tola_typst::typst::syntax::FileId;

use super::response::{ResponseBody, respond_bytes};
use crate::dev::site::CurrentSite;

pub(in crate::dev) type PublicationRequest =
    tokio::sync::oneshot::Sender<Result<Arc<SiteRevision>, usize>>;

#[derive(Serialize)]
struct PublishedRoute<'a> {
    output: &'a str,
    route: String,
}

#[derive(Serialize)]
struct Publication<'a> {
    revision: Option<&'a str>,
    routes: Vec<PublishedRoute<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

pub(super) async fn respond(
    request: Request<()>,
    peer: SocketAddr,
    sites: CurrentSite,
    initial_config: &ResolvedSiteConfig,
    publications: Option<tokio::sync::mpsc::Sender<PublicationRequest>>,
) -> Response<ResponseBody> {
    let method = request.method();
    if !peer.ip().is_loopback()
        || request.headers().contains_key(hyper::header::ORIGIN)
        || request
            .headers()
            .get("X-Tola-Preview")
            .and_then(|value| value.to_str().ok())
            != Some("1")
    {
        return reply(
            method,
            StatusCode::FORBIDDEN,
            None,
            None,
            Some("Tola preview requests require a local editor connection"),
        );
    }
    if !matches!(*method, Method::GET | Method::POST) {
        return reply(
            method,
            StatusCode::METHOD_NOT_ALLOWED,
            None,
            None,
            Some("Tola preview accepts GET or POST"),
        );
    }
    let previous = sites.revision();
    let config = previous
        .as_ref()
        .map_or(initial_config, |site| site.config().as_ref());
    let mut source = None;
    let mut expected_revision = None;
    for (name, value) in
        url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
    {
        match name.as_ref() {
            "source" if source.is_none() => match source_id(&value, config) {
                Some(id) => source = Some(id),
                None => {
                    return reply(
                        method,
                        StatusCode::BAD_REQUEST,
                        previous.as_deref(),
                        None,
                        Some("Tola preview source must be a file URI inside this site"),
                    );
                }
            },
            "revision" if expected_revision.is_none() => expected_revision = Some(value),
            _ => {
                return reply(
                    method,
                    StatusCode::BAD_REQUEST,
                    previous.as_deref(),
                    None,
                    Some("Tola preview request has an unknown or repeated parameter"),
                );
            }
        }
    }
    let site = if *method == Method::POST {
        let Some(publications) = publications else {
            return reply(
                method,
                StatusCode::SERVICE_UNAVAILABLE,
                previous.as_deref(),
                None,
                Some(
                    "Tola preview confirmation requires the development watcher; restart with --watch=true",
                ),
            );
        };
        let (send, receive) = tokio::sync::oneshot::channel();
        if publications.send(send).await.is_err() {
            return reply(
                method,
                StatusCode::SERVICE_UNAVAILABLE,
                previous.as_deref(),
                None,
                Some("Tola's development server is stopping; no new preview was opened"),
            );
        }
        match receive.await {
            Ok(Ok(site)) => Some(site),
            Ok(Err(errors)) => {
                let current = sites.revision();
                let message = format!(
                    "Tola could not publish the saved site ({errors} build errors). The previous preview is unchanged."
                );
                return reply(
                    method,
                    StatusCode::CONFLICT,
                    current.as_deref(),
                    None,
                    Some(&message),
                );
            }
            Err(_) => {
                return reply(
                    method,
                    StatusCode::SERVICE_UNAVAILABLE,
                    previous.as_deref(),
                    None,
                    Some(
                        "Tola's development server stopped before confirming publication; no new preview was opened",
                    ),
                );
            }
        }
    } else {
        previous
    };
    let Some(site) = site else {
        return reply(
            method,
            StatusCode::SERVICE_UNAVAILABLE,
            None,
            None,
            Some("Tola has not published this site; fix the build errors before opening a preview"),
        );
    };
    if expected_revision
        .as_deref()
        .is_some_and(|expected| expected != site.manifest().revision().as_str())
    {
        return reply(
            method,
            StatusCode::CONFLICT,
            Some(&site),
            None,
            Some(
                "Tola published another revision while choosing the page; run Preview again. No new preview was opened",
            ),
        );
    }
    reply(method, StatusCode::OK, Some(&site), source, None)
}

fn source_id(uri: &str, config: &ResolvedSiteConfig) -> Option<FileId> {
    let uri = url::Url::parse(uri).ok()?;
    if uri.scheme() != "file" || uri.query().is_some() || uri.fragment().is_some() {
        return None;
    }
    let path = uri.to_file_path().ok()?;
    let path = tola_build::filesystem::normalize_existing_prefix(&path);
    let root = tola_build::filesystem::normalize_existing_prefix(config.get_root());
    tola_typst::file_id_from_path(&path, &root)
}

fn reply(
    method: &Method,
    status: StatusCode,
    site: Option<&SiteRevision>,
    source: Option<FileId>,
    error: Option<&str>,
) -> Response<ResponseBody> {
    let routes = match site {
        Some(site) if status.is_success() => site
            .address()
            .pages()
            .into_iter()
            .filter(|page| source.is_none_or(|source| page.sources.contains(&source)))
            .map(|page| PublishedRoute {
                output: page.output.as_str(),
                route: site.config().url_mount().browser_path(&page.permalink),
            })
            .collect(),
        _ => Vec::new(),
    };
    let publication = Publication {
        revision: site.map(|site| site.manifest().revision().as_str()),
        routes,
        error,
    };
    let body = serde_json::to_vec(&publication).expect("published routes serialize");
    respond_bytes(
        method,
        status,
        Some("application/json"),
        ResponseBody::whole(Bytes::from(body)),
    )
}
