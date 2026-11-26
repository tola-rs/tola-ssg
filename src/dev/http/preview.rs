//! Editor access to published HTML routes and the saved-input publication fence.
//!
//! A `fragment` query filters pages known to contain both `source` and that HTML ID in the
//! same published revision; it does not prove the ID came from that source.

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
    req: Request<()>,
    peer: SocketAddr,
    sites: CurrentSite,
    publications: Option<tokio::sync::mpsc::Sender<PublicationRequest>>,
) -> Response<ResponseBody> {
    let method = req.method();
    let refuse = |status: StatusCode, site: Option<&SiteRevision>, msg: &str| {
        reply(method, status, site, None, None, Some(msg))
    };
    if !peer.ip().is_loopback()
        || req.headers().contains_key(hyper::header::ORIGIN)
        || req
            .headers()
            .get("X-Tola-Preview")
            .and_then(|val| val.to_str().ok())
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
    let prev = sites.revision();
    let mut src_path = None;
    let mut expected_rev = None;
    let mut fragment = None;
    for (name, val) in url::form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes())
    {
        match name.as_ref() {
            "source" if src_path.is_none() => match file_path(&val) {
                Some(path) => src_path = Some(path),
                None => {
                    return refuse(
                        StatusCode::BAD_REQUEST,
                        prev.as_deref(),
                        "Tola preview source must be a file URI used by this site",
                    );
                }
            },
            "revision" if expected_rev.is_none() => expected_rev = Some(val),
            "fragment" if fragment.is_none() && !val.is_empty() => fragment = Some(val),
            _ => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    prev.as_deref(),
                    "Tola preview request has an unknown or repeated parameter",
                );
            }
        }
    }
    if fragment.is_some() && src_path.is_none() {
        return refuse(
            StatusCode::BAD_REQUEST,
            prev.as_deref(),
            "Tola preview fragment requires a source file",
        );
    }
    let site = if *method == Method::POST {
        let Some(publications) = publications else {
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                prev.as_deref(),
                "Tola preview confirmation requires the development watcher; restart with --watch=true",
            );
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        if publications.send(tx).await.is_err() {
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                prev.as_deref(),
                "Tola's development server is stopping; no new preview was opened",
            );
        }
        match rx.await {
            Ok(Ok(site)) => Some(site),
            Ok(Err(errors)) => {
                let cur = sites.revision();
                let msg = format!(
                    "Tola could not publish the saved site ({errors} build errors). Fix the errors in Problems or Tola output, then open the preview again."
                );
                return refuse(StatusCode::CONFLICT, cur.as_deref(), &msg);
            }
            Err(_) => {
                return refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    prev.as_deref(),
                    "Tola's development server stopped before confirming publication; no new preview was opened",
                );
            }
        }
    } else {
        prev
    };
    let Some(site) = site else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            None,
            "Tola has not published this site; fix the build errors before opening a preview",
        );
    };
    if expected_rev
        .as_deref()
        .is_some_and(|expected| expected != site.manifest().revision().as_str())
    {
        return refuse(
            StatusCode::CONFLICT,
            Some(&site),
            "Tola published another revision while choosing the page; run Preview again. No new preview was opened",
        );
    }
    let src = match src_path {
        Some(path) => match source_ids(&path, &site) {
            Some(ids) => Some(ids),
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
        src.as_deref(),
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
    let root = tola_input::normalize_existing_prefix(site.config().root());
    let mut ids = Vec::new();
    if let Some(id) = tola_typst::file_id_from_path(&path, &root) {
        ids.push(id);
    }
    // Use the installed revision's selected package checks rather than live manifests.
    for id in site
        .input_observation()
        .package_checks()
        .iter()
        .filter(|check| check.was_selected())
        .filter_map(|check| {
            let dir = check.canonical_target()?;
            let rel = VirtualPath::virtualize(dir, &path).ok()?;
            Some(RootedPath::new(VirtualRoot::Package(check.package().clone()), rel).intern())
        })
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    (!ids.is_empty()).then_some(ids)
}

fn reply(
    method: &Method,
    status: StatusCode,
    site: Option<&SiteRevision>,
    src: Option<&[FileId]>,
    fragment: Option<&str>,
    err: Option<&str>,
) -> Response<ResponseBody> {
    let routes = match site {
        Some(site) if status.is_success() => site
            .address()
            .pages()
            .into_iter()
            .filter(|page| {
                src.is_none_or(|ids| ids.iter().any(|id| page.sources.contains(id)))
                    && fragment
                        .is_none_or(|frag| site.address().page_has_fragment(&page.output, frag))
            })
            .map(|page| PublishedRoute {
                output: page.output.as_str(),
                route: site.config().url_mount().browser_path(&page.permalink),
            })
            .collect(),
        _ => Vec::new(),
    };
    let pub_body = Publication {
        revision: site.map(|site| site.manifest().revision().as_str()),
        routes,
        error: err,
    };
    let buf = serde_json::to_vec(&pub_body).expect("published routes serialize");
    respond_bytes(
        method,
        status,
        Some("application/json"),
        ResponseBody::whole(Bytes::from(buf)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::body::Body;
    use tola_build::build::{BuildMode, BuildSession};

    fn published() -> (tempfile::TempDir, CurrentSite) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::dev::tests::write_site(
            dir.path(),
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
        std::fs::write(dir.path().join("content/shared.typ"), "Shared.").unwrap();
        let mut session = BuildSession::new();
        let (checked, _) =
            crate::dev::tests::checked_candidate(&mut session, cfg, BuildMode::Production);
        let sites = CurrentSite::new();
        sites.replace(&mut session, checked).unwrap();
        (dir, sites)
    }

    async fn peer_response(
        req: Request<()>,
        peer: SocketAddr,
        sites: CurrentSite,
    ) -> Response<ResponseBody> {
        respond(req, peer, sites, None).await
    }

    fn request(src: Option<&Path>, fragment: Option<&str>, rev: Option<&str>) -> Request<()> {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        if let Some(src) = src {
            query.append_pair("source", url::Url::from_file_path(src).unwrap().as_str());
        }
        if let Some(fragment) = fragment {
            query.append_pair("fragment", fragment);
        }
        if let Some(rev) = rev {
            query.append_pair("revision", rev);
        }
        Request::builder()
            .uri(format!("/_tola/preview?{}", query.finish()))
            .header("X-Tola-Preview", "1")
            .body(())
            .unwrap()
    }

    async fn json(resp: Response<ResponseBody>) -> serde_json::Value {
        let mut body = resp.into_body();
        let mut buf = Vec::new();
        while let Some(frame) =
            futures_util::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
        {
            if let Ok(data) = frame.unwrap().into_data() {
                buf.extend_from_slice(&data);
            }
        }
        serde_json::from_slice(&buf).unwrap()
    }

    #[tokio::test]
    async fn preview_fragment_and_auth() {
        let (dir, sites) = published();
        let src = dir.path().join("content/shared.typ");
        let peer = "127.0.0.1:1234".parse().unwrap();

        let same =
            peer_response(request(Some(&src), Some("same"), None), peer, sites.clone()).await;
        assert_eq!(same.status(), StatusCode::OK);
        let val = json(same).await;
        let routes = val["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0]["route"], "/docs/one/");
        assert_eq!(routes[1]["route"], "/docs/two/");
        let rev = val["revision"].as_str().unwrap();

        let one = peer_response(
            request(Some(&src), Some("only-one"), Some(rev)),
            peer,
            sites.clone(),
        )
        .await;
        let rows = json(one).await;
        assert_eq!(rows["routes"].as_array().unwrap().len(), 1);
        assert_eq!(rows["routes"][0]["route"], "/docs/one/");

        let absent = peer_response(
            request(Some(&src), Some("missing"), Some(rev)),
            peer,
            sites.clone(),
        )
        .await;
        assert!(json(absent).await["routes"].as_array().unwrap().is_empty());

        let stale = peer_response(
            request(Some(&src), Some("same"), Some("stale")),
            peer,
            sites.clone(),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);

        assert_eq!(
            peer_response(request(None, Some("same"), None), peer, sites.clone())
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            peer_response(request(Some(&src), Some(""), None), peer, sites.clone())
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        let mut origin = request(Some(&src), Some("same"), None);
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
            request(Some(&src), Some("same"), None),
            "192.0.2.1:1234".parse().unwrap(),
            sites,
            None,
        )
        .await;
        assert_eq!(remote.status(), StatusCode::FORBIDDEN);
    }
}
