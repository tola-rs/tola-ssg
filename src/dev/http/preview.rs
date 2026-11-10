//! Editor access to published HTML routes and the saved-input publication fence.
//!
//! A `fragment` query is an explicit HTML ID. It requires `source` and filters pages
//! already known to contain that source and ID in the same published revision. It does
//! not assert that the ID was produced by that source or identify a cursor occurrence.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;
use tola_build::site::SiteRevision;
use tola_typst::typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

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
    publications: Option<tokio::sync::mpsc::Sender<PublicationRequest>>,
) -> Response<ResponseBody> {
    let method = request.method();
    // Every refusal below names one status and one sentence for the editor to show.
    let refuse = |status: StatusCode, site: Option<&SiteRevision>, message: &str| {
        reply(method, status, site, None, None, Some(message))
    };
    if !peer.ip().is_loopback()
        || request.headers().contains_key(hyper::header::ORIGIN)
        || request
            .headers()
            .get("X-Tola-Preview")
            .and_then(|value| value.to_str().ok())
            != Some("1")
    {
        return refuse(
            StatusCode::FORBIDDEN,
            None,
            "Tola preview requests require a local editor connection",
        );
    }
    if !matches!(*method, Method::GET | Method::POST) {
        return refuse(
            StatusCode::METHOD_NOT_ALLOWED,
            None,
            "Tola preview accepts GET or POST",
        );
    }
    let previous = sites.revision();
    let mut source_path = None;
    let mut expected_revision = None;
    let mut fragment = None;
    for (name, value) in
        url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
    {
        match name.as_ref() {
            "source" if source_path.is_none() => match file_path(&value) {
                Some(path) => source_path = Some(path),
                None => {
                    return refuse(
                        StatusCode::BAD_REQUEST,
                        previous.as_deref(),
                        "Tola preview source must be a file URI used by this site",
                    );
                }
            },
            "revision" if expected_revision.is_none() => expected_revision = Some(value),
            "fragment" if fragment.is_none() && !value.is_empty() => fragment = Some(value),
            _ => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    previous.as_deref(),
                    "Tola preview request has an unknown or repeated parameter",
                );
            }
        }
    }
    if fragment.is_some() && source_path.is_none() {
        return refuse(
            StatusCode::BAD_REQUEST,
            previous.as_deref(),
            "Tola preview fragment requires a source file",
        );
    }
    let site = if *method == Method::POST {
        let Some(publications) = publications else {
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                previous.as_deref(),
                "Tola preview confirmation requires the development watcher; restart with --watch=true",
            );
        };
        let (send, receive) = tokio::sync::oneshot::channel();
        if publications.send(send).await.is_err() {
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                previous.as_deref(),
                "Tola's development server is stopping; no new preview was opened",
            );
        }
        match receive.await {
            Ok(Ok(site)) => Some(site),
            Ok(Err(errors)) => {
                let current = sites.revision();
                let message = format!(
                    "Tola could not publish the saved site ({errors} build errors). The previous preview is unchanged."
                );
                return refuse(StatusCode::CONFLICT, current.as_deref(), &message);
            }
            Err(_) => {
                return refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    previous.as_deref(),
                    "Tola's development server stopped before confirming publication; no new preview was opened",
                );
            }
        }
    } else {
        previous
    };
    let Some(site) = site else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            None,
            "Tola has not published this site; fix the build errors before opening a preview",
        );
    };
    if expected_revision
        .as_deref()
        .is_some_and(|expected| expected != site.manifest().revision().as_str())
    {
        return refuse(
            StatusCode::CONFLICT,
            Some(&site),
            "Tola published another revision while choosing the page; run Preview again. No new preview was opened",
        );
    }
    let source = match source_path {
        Some(path) => match source_ids(&path, &site) {
            Some(source) => Some(source),
            None => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    Some(&site),
                    "Tola preview source must be a file URI used by this site",
                );
            }
        },
        None => None,
    };
    reply(
        method,
        StatusCode::OK,
        Some(&site),
        source.as_deref(),
        fragment.as_deref(),
        None,
    )
}

fn file_path(uri: &str) -> Option<PathBuf> {
    let uri = url::Url::parse(uri).ok()?;
    if uri.scheme() != "file" || uri.query().is_some() || uri.fragment().is_some() {
        return None;
    }
    uri.to_file_path().ok()
}

fn source_ids(path: &Path, site: &SiteRevision) -> Option<Vec<FileId>> {
    let path = tola_input::normalize_existing_prefix(path);
    let root = tola_input::normalize_existing_prefix(site.config().get_root());
    let mut sources = Vec::new();
    if let Some(id) = tola_typst::file_id_from_path(&path, &root) {
        sources.push(id);
    }
    // One physical source can contribute pages through both Project and selected Package IDs.
    // The installed revision's checks, not current manifests or search order, name those IDs.
    for id in site
        .input_observation()
        .package_checks()
        .iter()
        .filter(|check| check.was_selected())
        .filter_map(|check| {
            let directory = check.canonical_target()?;
            let relative = VirtualPath::virtualize(directory, &path).ok()?;
            Some(RootedPath::new(VirtualRoot::Package(check.package().clone()), relative).intern())
        })
    {
        if !sources.contains(&id) {
            sources.push(id);
        }
    }
    (!sources.is_empty()).then_some(sources)
}

fn reply(
    method: &Method,
    status: StatusCode,
    site: Option<&SiteRevision>,
    source: Option<&[FileId]>,
    fragment: Option<&str>,
    error: Option<&str>,
) -> Response<ResponseBody> {
    let routes = match site {
        Some(site) if status.is_success() => site
            .address()
            .pages()
            .into_iter()
            .filter(|page| {
                source.is_none_or(|sources| sources.iter().any(|id| page.sources.contains(id)))
                    && fragment.is_none_or(|fragment| {
                        site.address().page_has_fragment(&page.output, fragment)
                    })
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::body::Body;
    use tola_build::build::{BuildMode, BuildSession};

    fn published() -> (tempfile::TempDir, CurrentSite) {
        let root = tempfile::tempdir().unwrap();
        let config = crate::dev::tests::write_site(
            root.path(),
            r#"
#document("one/index.html", format: "html")[
  #context { include "content/shared.typ" }
  #html.elem("span", attrs: (id: "same"))[]
  #html.elem("span", attrs: (id: "only-one"))[]
]
#document("two/index.html", format: "html")[
  #context { include "content/shared.typ" }
  #html.elem("span", attrs: (id: "same"))[]
]
#document("unrelated/index.html", format: "html")[#html.elem("span", attrs: (id: "only-one"))[]]
"#,
            "[site]\nbase-path = '/docs/'\n",
        );
        std::fs::write(root.path().join("content/shared.typ"), "Shared.").unwrap();
        let mut session = BuildSession::new();
        let (checked, _) =
            crate::dev::tests::checked_candidate(&mut session, config, BuildMode::Production);
        let sites = CurrentSite::new();
        sites.replace(&mut session, checked).unwrap();
        (root, sites)
    }

    async fn peer_response(
        request: Request<()>,
        peer: SocketAddr,
        sites: CurrentSite,
    ) -> Response<ResponseBody> {
        respond(request, peer, sites, None).await
    }

    fn request(
        source: Option<&Path>,
        fragment: Option<&str>,
        revision: Option<&str>,
    ) -> Request<()> {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        if let Some(source) = source {
            query.append_pair("source", url::Url::from_file_path(source).unwrap().as_str());
        }
        if let Some(fragment) = fragment {
            query.append_pair("fragment", fragment);
        }
        if let Some(revision) = revision {
            query.append_pair("revision", revision);
        }
        Request::builder()
            .uri(format!("/_tola/preview?{}", query.finish()))
            .header("X-Tola-Preview", "1")
            .body(())
            .unwrap()
    }

    async fn json(response: Response<ResponseBody>) -> serde_json::Value {
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) =
            futures_util::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
        {
            if let Ok(data) = frame.unwrap().into_data() {
                bytes.extend_from_slice(&data);
            }
        }
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn fragment_published_pages_source_revision() {
        let (root, sites) = published();
        let source = root.path().join("content/shared.typ");
        let peer = "127.0.0.1:1234".parse().unwrap();
        let same = peer_response(
            request(Some(&source), Some("same"), None),
            peer,
            sites.clone(),
        )
        .await;
        assert_eq!(same.status(), StatusCode::OK);
        let value = json(same).await;
        let routes = value["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0]["route"], "/docs/one/");
        assert_eq!(routes[1]["route"], "/docs/two/");
        let revision = value["revision"].as_str().unwrap();
        let one = peer_response(
            request(Some(&source), Some("only-one"), Some(revision)),
            peer,
            sites.clone(),
        )
        .await;
        let rows = json(one).await;
        assert_eq!(rows["routes"].as_array().unwrap().len(), 1);
        assert_eq!(rows["routes"][0]["route"], "/docs/one/");
        let absent = peer_response(
            request(Some(&source), Some("missing"), Some(revision)),
            peer,
            sites.clone(),
        )
        .await;
        assert!(json(absent).await["routes"].as_array().unwrap().is_empty());
        let stale = peer_response(
            request(Some(&source), Some("same"), Some("stale")),
            peer,
            sites.clone(),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let missing_source =
            peer_response(request(None, Some("same"), None), peer, sites.clone()).await;
        assert_eq!(missing_source.status(), StatusCode::BAD_REQUEST);
        let empty =
            peer_response(request(Some(&source), Some(""), None), peer, sites.clone()).await;
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
        let mut origin = request(Some(&source), Some("same"), None);
        origin.headers_mut().insert(
            hyper::header::ORIGIN,
            "https://example.com".parse().unwrap(),
        );
        assert_eq!(
            peer_response(origin, peer, sites.clone()).await.status(),
            StatusCode::FORBIDDEN
        );
        let duplicate = Request::builder()
            .uri("/_tola/preview?fragment=a&fragment=b")
            .header("X-Tola-Preview", "1")
            .body(())
            .unwrap();
        assert_eq!(
            peer_response(duplicate, peer, sites.clone()).await.status(),
            StatusCode::BAD_REQUEST
        );
        let remote = respond(
            request(Some(&source), Some("same"), None),
            "192.0.2.1:1234".parse().unwrap(),
            sites,
            None,
        )
        .await;
        assert_eq!(remote.status(), StatusCode::FORBIDDEN);
    }
}
