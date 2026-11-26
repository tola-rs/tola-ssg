use std::fs;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use hyper::{Method, Request, Response, StatusCode};
use tempfile::TempDir;
use tola_address::SiteUrlMount;
use tola_build::output::semantics::ResponseMediaType;

use super::super::tests::site_config;
use super::*;

fn request(url: &str) -> Request<()> {
    request_with_method(Method::GET, url)
}

fn request_with_method(method: Method, url: &str) -> Request<()> {
    Request::builder().method(method).uri(url).body(()).unwrap()
}

fn request_with_header(method: Method, url: &str, name: &'static str, val: &str) -> Request<()> {
    let mut hdrs = hyper::HeaderMap::new();
    hdrs.insert(
        hyper::header::HeaderName::from_static(name),
        hyper::header::HeaderValue::from_str(val).unwrap(),
    );
    let mut req = request_with_method(method, url);
    *req.headers_mut() = hdrs;
    req
}

fn reload_endpoint() -> ReloadEndpoint {
    ReloadEndpoint::new(35729, "test-session", "test-generation")
}

fn site_outputs(program: &str, assets: &[(&str, &str, &[u8])]) -> OutputRevision {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let files = assets
        .iter()
        .map(|(path, src, bytes)| {
            fs::write(root.join(src), bytes).unwrap();
            format!(
                "{{source = {}, url = {}}}",
                serde_json::to_string(src).unwrap(),
                serde_json::to_string(&format!("/{path}")).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let cfg =
        format!("[build.minify]\ncss = false\njavascript = false\n[assets]\nfiles = [{files}]\n");
    crate::dev::tests::built_outputs(
        &crate::dev::tests::write_site(root, program, &cfg),
        tola_build::build::BuildMode::Production,
    )
}

fn document_program(path: &str, body: &str) -> String {
    format!(
        "#document({})[{body}]",
        serde_json::to_string(path).unwrap()
    )
}

fn output<'a>(rev: &'a OutputRevision, path: &str) -> &'a OutputFile {
    rev.output(&OutputPath::parse(path).unwrap()).unwrap()
}

fn published_response(
    req: Request<()>,
    site: &Arc<InstalledRevision>,
    endpoint: Option<&ReloadEndpoint>,
) -> Response<ResponseBody> {
    respond_published(req, site, endpoint)
}

fn file_response(
    req: Request<()>,
    file: &OutputFile,
    rev: &OutputRevision,
    endpoint: Option<&ReloadEndpoint>,
) -> Response<ResponseBody> {
    respond_file(
        req,
        file,
        &SiteUrlMount::root(),
        endpoint,
        rev,
        &HtmlResponses::default(),
    )
}

fn content_length(resp: Response<ResponseBody>) -> u64 {
    resp.headers()["content-length"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn body_bytes(body: &ResponseBody) -> Bytes {
    body.remaining_slices()
        .iter()
        .flat_map(|slice| slice.iter().copied())
        .collect::<Vec<u8>>()
        .into()
}

fn published_site(root: &Path, program: &str, cfg: &str) -> Arc<InstalledRevision> {
    let mut session = tola_build::build::BuildSession::new();
    let (checked, _) = crate::dev::tests::checked_candidate(
        &mut session,
        crate::dev::tests::write_site(root, program, cfg),
        tola_build::build::BuildMode::Production,
    );
    Arc::new(InstalledRevision::new(Arc::new(
        session.install_revision(checked, Ok).unwrap(),
    )))
}

fn hot_reload_request_with_method(method: Method, rev: &str) -> Request<()> {
    let mut hdrs = hyper::HeaderMap::new();
    hdrs.insert(
        hyper::header::HeaderName::from_static("x-tola-hot-reload"),
        HeaderValue::from_static("true"),
    );
    hdrs.insert(
        hyper::header::HeaderName::from_static("x-tola-revision"),
        HeaderValue::from_str(rev).unwrap(),
    );
    let mut req = Request::builder().method(method).uri("/").body(()).unwrap();
    *req.headers_mut() = hdrs;
    req
}

#[test]
fn unusable_ranges_serve_whole() {
    let rev = site_outputs("", &[("download.bin", "input.bin", b"0123456789")]);
    let mut conditional = request_with_header(Method::GET, "/download.bin", "range", "bytes=2-5");
    conditional
        .headers_mut()
        .insert(IF_RANGE, HeaderValue::from_static("\"previous-revision\""));

    for req in [
        conditional,
        request_with_header(Method::GET, "/download.bin", "range", "items=2-5"),
    ] {
        let resp = file_response(req, output(&rev, "download.bin"), &rev, None);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_bytes(resp.body()).as_ref(), b"0123456789");
        assert!(!resp.headers().contains_key(CONTENT_RANGE));
    }
}

#[test]
fn head_reports_get_length() {
    let rev = site_outputs("", &[("media/video.mp4", "input.mp4", b"0123456789")]);
    let asset = output(&rev, "media/video.mp4");
    let asset_get = file_response(
        request("/media/video.mp4"),
        asset,
        &rev,
        Some(&reload_endpoint()),
    );
    let asset_head = file_response(
        request_with_header(Method::HEAD, "/media/video.mp4", "range", "bytes=2-5"),
        asset,
        &rev,
        Some(&reload_endpoint()),
    );
    assert_eq!(
        content_length(file_response(
            request_with_header(Method::GET, "/media/video.mp4", "range", "bytes=2-5"),
            asset,
            &rev,
            Some(&reload_endpoint()),
        )),
        4
    );

    let page = site_outputs(&document_program("index.html", "Page"), &[]);
    let page_get = file_response(
        request("/index.html"),
        output(&page, "index.html"),
        &page,
        Some(&reload_endpoint()),
    );
    let page_head = file_response(
        request_with_method(Method::HEAD, "/index.html"),
        output(&page, "index.html"),
        &page,
        Some(&reload_endpoint()),
    );

    let runtime_get = respond_hotreload_js(request("/hotreload.js"));
    let runtime_head = respond_hotreload_js(request_with_method(Method::HEAD, "/hotreload.js"));

    let err_get = text_status_response(&Method::GET, StatusCode::BAD_REQUEST, "400 Bad Request");
    let err_head = text_status_response(&Method::HEAD, StatusCode::BAD_REQUEST, "400 Bad Request");
    assert_eq!(body_bytes(err_get.body()).as_ref(), b"400 Bad Request");

    for (get, head) in [
        (asset_get, asset_head),
        (page_get, page_head),
        (runtime_get, runtime_head),
        (err_get, err_head),
    ] {
        assert_eq!(head.status(), get.status());
        assert!(body_bytes(head.body()).is_empty());
        assert_eq!(content_length(head), body_bytes(get.body()).len() as u64);
    }
}

#[test]
fn stale_representation_conflict() {
    let rev = site_outputs("", &[("styles/site.css", "input.css", b"body {}")]);
    let cur = output(&rev, "styles/site.css");
    let repr = tola_build::output::manifest::RepresentationId::from_output(cur).to_hex();
    let accepted = file_response(
        request(&format!("/styles/site.css?tola-representation={repr}")),
        cur,
        &rev,
        Some(&reload_endpoint()),
    );
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(body_bytes(accepted.body()).as_ref(), b"body {}");

    let unknown = file_response(
        request("/styles/site.css?tola-representation=stale"),
        cur,
        &rev,
        Some(&reload_endpoint()),
    );
    assert_eq!(unknown.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_bytes(unknown.body()).as_ref(),
        b"the site changed; reload the page"
    );

    let head = file_response(
        request_with_method(Method::HEAD, "/styles/site.css?tola-representation=stale"),
        cur,
        &rev,
        Some(&reload_endpoint()),
    );
    assert_eq!(head.status(), StatusCode::CONFLICT);
    assert!(body_bytes(head.body()).is_empty());

    let before = site_outputs("", &[("styles/site", "input.bin", b"body {}")]);
    let after = site_outputs("", &[("styles/site", "input.css", b"body {}")]);
    let prev =
        tola_build::output::manifest::RepresentationId::from_output(output(&before, "styles/site"))
            .to_hex();
    let changed = file_response(
        request(&format!("/styles/site?tola-representation={prev}")),
        output(&after, "styles/site"),
        &after,
        Some(&reload_endpoint()),
    );
    assert_eq!(changed.status(), StatusCode::CONFLICT);
}

#[test]
fn user_representation_ignored() {
    let rev = site_outputs("", &[("styles/site.css", "input.css", b"body {}")]);
    let resp = file_response(
        request("/styles/site.css?tola-representation=user-value"),
        output(&rev, "styles/site.css"),
        &rev,
        None,
    );
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp.body()).as_ref(), b"body {}");
}

#[test]
fn extensionless_asset_media_type() {
    let rev = site_outputs("", &[("styles/app", "input.css", b"body {}")]);
    let file = output(&rev, "styles/app");
    for resp in [
        file_response(request("/styles/app"), file, &rev, None),
        file_response(
            request_with_method(Method::HEAD, "/styles/app"),
            file,
            &rev,
            None,
        ),
        file_response(
            request_with_header(Method::GET, "/styles/app", "range", "bytes=0-3"),
            file,
            &rev,
            None,
        ),
    ] {
        assert_eq!(
            resp.headers()[CONTENT_TYPE],
            ResponseMediaType::CSS.as_str()
        );
    }
}

#[test]
fn html_without_reload_verbatim() {
    let rev = site_outputs(&document_program("index.html", "exact"), &[]);
    let resp = file_response(
        request("/index.html"),
        output(&rev, "index.html"),
        &rev,
        None,
    );
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(resp.body()).as_ref(),
        output(&rev, "index.html").bytes()
    );
}

#[test]
fn duplicate_representation_bad_request() {
    let rev = site_outputs("", &[("site.css", "input.css", b"body {}")]);
    let resp = file_response(
        request("/site.css?tola-representation=first&tola-representation=second"),
        output(&rev, "site.css"),
        &rev,
        Some(&reload_endpoint()),
    );
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_bytes(resp.body()).as_ref(), b"400 Bad Request");
}

#[test]
fn range_spans_injected_slices() {
    let rev = site_outputs(&document_program("index.html", "Home"), &[]);
    let endpoint = reload_endpoint();
    let file = output(&rev, "index.html");
    let whole = file_response(request("/"), file, &rev, Some(&endpoint));
    let partial = file_response(
        request_with_header(Method::GET, "/", "range", "bytes=7-40"),
        file,
        &rev,
        Some(&endpoint),
    );
    let whole = body_bytes(whole.body());
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_bytes(partial.body()).as_ref(), &whole[7..41]);
}

#[test]
fn dev_responses_no_store() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("download.txt"), "download").unwrap();
    let site = published_site(
        dir.path(),
        "",
        "[assets]\nfiles = [{source = \"download.txt\", url = \"/download\"}]",
    );
    let cfg = site.config();
    let endpoint = reload_endpoint();
    let unpublished = HtmlResponses::default();
    let runtime = crate::embed::dev::hotreload_browser_path(cfg.url_mount());

    for (req, status) in [
        (request("/"), StatusCode::OK),
        (request("/download"), StatusCode::OK),
        (
            request_with_header(Method::GET, "/download", "range", "bytes=0-3"),
            StatusCode::PARTIAL_CONTENT,
        ),
        (
            request_with_header(Method::GET, "/download", "range", "bytes=99-"),
            StatusCode::RANGE_NOT_SATISFIABLE,
        ),
        (request("/missing"), StatusCode::NOT_FOUND),
        (request(&runtime), StatusCode::OK),
    ] {
        let resp = respond(
            req,
            Some(Arc::clone(&site)),
            cfg,
            Some(&endpoint),
            &unpublished,
        );
        assert_eq!(resp.status(), status);
        assert!(
            resp.headers()[CACHE_CONTROL]
                .to_str()
                .unwrap()
                .split(',')
                .any(|val| val.trim() == "no-store")
        );
    }

    let failed = respond(request("/"), None, cfg, Some(&endpoint), &unpublished);
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        failed.headers()[CACHE_CONTROL]
            .to_str()
            .unwrap()
            .split(',')
            .any(|val| val.trim() == "no-store")
    );
}

#[test]
fn method_not_allowed_allow_header() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = site_config(dir.path(), "");
    let resp = respond(
        request_with_method(Method::POST, "/"),
        None,
        &cfg,
        None,
        &HtmlResponses::default(),
    );
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body_bytes(resp.body()).as_ref(), b"405 Method Not Allowed");
    assert_eq!(resp.headers()[ALLOW], "GET, HEAD");
}

#[test]
fn unsatisfiable_range_content_range() {
    let rev = site_outputs("", &[("media/video.mp4", "input.mp4", b"0123456789")]);
    let resp = file_response(
        request_with_header(Method::GET, "/media/video.mp4", "range", "bytes=10-"),
        output(&rev, "media/video.mp4"),
        &rev,
        None,
    );
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(resp.headers()[CONTENT_RANGE], "bytes */10");
    assert!(body_bytes(resp.body()).is_empty());
    assert_eq!(content_length(resp), 0);
}

#[test]
fn empty_site_serves_welcome() {
    for (cfg, root_path) in [("", "/"), ("[site]\nbase-path = \"/docs/\"", "/docs/")] {
        let dir = TempDir::new().unwrap();
        let site = published_site(dir.path(), "", cfg);
        let diag = tola_build::build::no_pages_diagnostic(site.config());
        let endpoint = reload_endpoint();
        let get = published_response(request(root_path), &site, Some(&endpoint));
        let head = published_response(
            request_with_method(Method::HEAD, root_path),
            &site,
            Some(&endpoint),
        );

        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(
            get.headers()[CONTENT_TYPE],
            ResponseMediaType::HTML.as_str()
        );
        let body = body_bytes(get.body());
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains(tola_build::html::escape(&diag.message).as_ref()));
        assert!(html.contains(tola_build::html::escape(&diag.help[0].message).as_ref()));
        assert!(html.contains("data-tola-runtime"));
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[CONTENT_TYPE], get.headers()[CONTENT_TYPE]);
        assert!(body_bytes(head.body()).is_empty());
        assert_eq!(content_length(head), body.len() as u64);
    }
}

#[test]
fn runtime_minify_error_reported() {
    let err = crate::embed::dev::RuntimeMinifyError::RuntimeSource("deliberate failure".into());
    let resp = respond_development_runtime(&Method::GET, Err(&err));
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = String::from_utf8_lossy(&body_bytes(resp.body())).into_owned();
    assert!(
        body.contains("could not prepare its development runtime"),
        "{body}"
    );
    assert!(!body.contains("deliberate failure"));
}

#[test]
fn exact_paths_and_indexes() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("download.txt"), "download").unwrap();
    let site = published_site(
        dir.path(),
        "#document(\"guide/index.html\")[Guide]\n#document(\"404.html\")[Missing]",
        "[assets]\nfiles = [{source = \"download.txt\", url = \"/download\"}]",
    );

    for (path, status, output_path) in [
        ("/guide/", StatusCode::OK, "guide/index.html"),
        ("/guide/index.html?x=1", StatusCode::OK, "guide/index.html"),
        ("/guide", StatusCode::NOT_FOUND, "404.html"),
        ("/download", StatusCode::OK, "download"),
        ("/missing", StatusCode::NOT_FOUND, "404.html"),
        ("/404.html", StatusCode::NOT_FOUND, "404.html"),
    ] {
        for method in [Method::GET, Method::HEAD] {
            let resp = published_response(request_with_method(method.clone(), path), &site, None);
            assert_eq!(resp.status(), status, "{method} {path}");
            assert!(!resp.headers().contains_key(hyper::header::LOCATION));
            if method == Method::HEAD {
                assert!(body_bytes(resp.body()).is_empty());
            } else {
                assert_eq!(
                    body_bytes(resp.body()).as_ref(),
                    output(site.outputs(), output_path).bytes()
                );
            }
        }
    }
}

#[test]
fn mounted_unicode_route() {
    let dir = TempDir::new().unwrap();
    let site = published_site(
        dir.path(),
        "#document(\"index.html\")[Home]\n#document(\"release..notes/中文/index.html\")[Guide]",
        "[site]\nbase-path = \"/文档/\"",
    );

    let resp = published_response(
        request("/%e6%96%87%e6%a1%a3/release..notes/%e4%b8%ad%e6%96%87/"),
        &site,
        None,
    );

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(resp.body()).as_ref(),
        output(site.outputs(), "release..notes/中文/index.html").bytes()
    );
}

#[test]
fn generation_header_public() {
    let dir = TempDir::new().unwrap();
    let site = published_site(dir.path(), &document_program("index.html", "unchanged"), "");
    let cfg = site.config();
    let first = ReloadEndpoint::new(35729, "secret-first", "public-first");
    let restarted = ReloadEndpoint::new(35729, "secret-next", "public-next");
    let call = |req, endpoint| {
        respond(
            req,
            Some(Arc::clone(&site)),
            cfg,
            Some(endpoint),
            &HtmlResponses::default(),
        )
    };
    let original = call(request_with_method(Method::HEAD, "/"), &first);
    let next = call(request_with_method(Method::HEAD, "/"), &restarted);
    assert_eq!(original.status(), StatusCode::OK);
    assert_eq!(next.status(), StatusCode::OK);
    assert_ne!(
        original.headers()["X-Tola-Reload-Generation"],
        next.headers()["X-Tola-Reload-Generation"]
    );
    assert!(body_bytes(next.body()).is_empty());
    assert!(next.headers().values().all(|val| {
        !val.as_bytes()
            .windows(b"secret-".len())
            .any(|part| part == b"secret-")
    }));

    let unavailable = respond(
        request_with_method(Method::HEAD, "/"),
        None,
        cfg,
        Some(&restarted),
        &HtmlResponses::default(),
    );
    let missing = call(request_with_method(Method::HEAD, "/removed"), &restarted);
    let fenced = call(
        hot_reload_request_with_method(Method::HEAD, "stale"),
        &restarted,
    );
    let fenced_get = call(
        hot_reload_request_with_method(Method::GET, "stale"),
        &restarted,
    );
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(fenced.status(), StatusCode::CONFLICT);
    assert_eq!(fenced_get.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_bytes(fenced_get.body()).as_ref(),
        b"the site changed; reload the page"
    );
    for resp in [unavailable, missing, fenced] {
        assert_eq!(
            resp.headers()["X-Tola-Reload-Generation"],
            next.headers()["X-Tola-Reload-Generation"]
        );
        assert!(body_bytes(resp.body()).is_empty());
    }
}

#[test]
fn missing_routes_404_status() {
    for (cfg, missing_paths) in [
        (
            "",
            ["/", "/404.html", "/missing", "/index.html", "/nested/"],
        ),
        (
            "[site]\nbase-path = \"/docs/\"",
            [
                "/docs/",
                "/docs/404.html",
                "/docs/missing",
                "/docs/index.html",
                "/",
            ],
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let site = published_site(dir.path(), "#document(\"404.html\")[Not Found]", cfg);
        for path in missing_paths {
            let get = published_response(request(path), &site, None);
            let head = published_response(request_with_method(Method::HEAD, path), &site, None);
            assert_eq!(get.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                body_bytes(get.body()).as_ref(),
                output(site.outputs(), "404.html").bytes(),
                "{path}"
            );
            assert_eq!(head.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(body_bytes(head.body()).is_empty(), "{path}");
            assert_eq!(content_length(head), body_bytes(get.body()).len() as u64);
        }
    }
}

#[test]
fn asset_404_served_ok() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("not-found.html"), "ordinary asset").unwrap();
    let site = published_site(
        dir.path(),
        "",
        "[assets]\nfiles = [{source = \"not-found.html\", url = \"/404.html\"}]",
    );
    let resp = published_response(request("/404.html"), &site, Some(&reload_endpoint()));
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp.body()).as_ref(), b"ordinary asset");
}

#[test]
fn non_html_site_statuses() {
    for program in ["", "#document(\"download.pdf\", format: \"pdf\")[Download]"] {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("theme.css"), "body {}").unwrap();
        let site = published_site(
            dir.path(),
            program,
            "[build.minify]\ncss = false\n[assets]\nfiles = [{ source = \"theme.css\", url = \"/theme.css\" }]",
        );
        for (path, status) in [
            ("/", StatusCode::OK),
            ("/theme.css", StatusCode::OK),
            ("/missing", StatusCode::NOT_FOUND),
        ] {
            let get = published_response(request(path), &site, None);
            let head = published_response(request_with_method(Method::HEAD, path), &site, None);
            assert_eq!(get.status(), status, "{program}: {path}");
            assert_eq!(head.status(), status, "{program}: {path}");
            assert!(body_bytes(head.body()).is_empty());
            assert_eq!(content_length(head), body_bytes(get.body()).len() as u64);
            if path == "/theme.css" {
                assert_eq!(body_bytes(get.body()).as_ref(), b"body {}");
            } else if path == "/missing" {
                assert_eq!(body_bytes(get.body()).as_ref(), b"404 Not Found");
            } else {
                let body = body_bytes(get.body());
                assert!(
                    String::from_utf8_lossy(&body).contains(
                        tola_build::html::escape(
                            &tola_build::build::no_pages_diagnostic(site.config()).message
                        )
                        .as_ref()
                    )
                );
            }
        }
    }
}
