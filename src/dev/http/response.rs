//! Routing and HTTP representations of one published development revision.

use std::sync::Arc;

use crate::dev::site::{HtmlResponses, InstalledRevision};
use anyhow::Result;
use bytes::Bytes;
use hyper::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, EXPIRES,
    HeaderValue, IF_RANGE, PRAGMA, RANGE,
};
use hyper::{Method, Request, Response, StatusCode};
use tola_address::{SiteUrlMount, UrlPath};
use tola_build::config::ResolvedSiteConfig;
use tola_build::diagnostic::Diagnostic;
use tola_build::output::graph::{OutputFile, OutputKind};
use tola_build::output::revision::OutputRevision;

use super::html::HtmlInjection;
use crate::dev::reload::transport::ReloadEndpoint;

#[derive(Debug)]
enum BodySlices {
    Whole(Bytes),
    Fragments([Bytes; 3]),
    HeadMetadata,
}

#[derive(Debug)]
pub(in crate::dev) struct ResponseBody {
    slices: BodySlices,
    next: usize,
    length: usize,
}

impl ResponseBody {
    pub(super) fn whole(bytes: Bytes) -> Self {
        let length = bytes.len();
        Self {
            slices: BodySlices::Whole(bytes),
            next: 0,
            length,
        }
    }

    pub(in crate::dev) fn slices(slices: [Bytes; 3]) -> Self {
        let length = slices.iter().map(Bytes::len).sum();
        Self {
            slices: BodySlices::Fragments(slices),
            next: 0,
            length,
        }
    }

    pub(in crate::dev) fn head_metadata(length: usize) -> Self {
        Self {
            slices: BodySlices::HeadMetadata,
            next: 0,
            length,
        }
    }

    fn remaining_slices(&self) -> &[Bytes] {
        let slices = match &self.slices {
            BodySlices::Whole(bytes) => std::slice::from_ref(bytes),
            BodySlices::Fragments(slices) => slices.as_slice(),
            BodySlices::HeadMetadata => &[],
        };
        &slices[self.next..]
    }

    fn byte_len(&self) -> usize {
        self.length
    }

    fn within(&self, start: usize, end: usize) -> Option<Self> {
        let mut slices = std::array::from_fn(|_| Bytes::new());
        let mut count = 0;
        let mut offset = 0;
        for slice in self.remaining_slices() {
            if offset > end {
                break;
            }
            let next = offset + slice.len();
            if next > start {
                let from = start.saturating_sub(offset);
                let to = (end + 1 - offset).min(slice.len());
                slices[count] = slice.slice(from..to);
                count += 1;
            }
            offset = next;
        }
        match count {
            0 => None,
            1 => Some(Self::whole(std::mem::take(&mut slices[0]))),
            _ => Some(Self::slices(slices)),
        }
    }
}

impl hyper::body::Body for ResponseBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<std::result::Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        let body = self.get_mut();
        let bytes = match &mut body.slices {
            BodySlices::Whole(bytes) if body.next == 0 => Some(std::mem::take(bytes)),
            BodySlices::Fragments(slices) => slices.get_mut(body.next).map(std::mem::take),
            BodySlices::Whole(_) | BodySlices::HeadMetadata => None,
        };
        if let Some(bytes) = bytes {
            body.next += 1;
            body.length -= bytes.len();
            std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(bytes))))
        } else {
            std::task::Poll::Ready(None)
        }
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        hyper::body::SizeHint::with_exact(if matches!(self.slices, BodySlices::HeadMetadata) {
            0
        } else {
            self.byte_len() as u64
        })
    }
}

pub(super) fn respond(
    request: Request<()>,
    site: Option<Arc<InstalledRevision>>,
    initial_config: &ResolvedSiteConfig,
    reload_endpoint: Option<&ReloadEndpoint>,
    unpublished_html: &HtmlResponses,
) -> Response<ResponseBody> {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return respond_method_not_allowed(request.method());
    }
    let config = site
        .as_ref()
        .map_or(initial_config, |site| site.config().as_ref());
    if reload_endpoint.is_some()
        && request.uri().path() == crate::embed::dev::hotreload_browser_path(&config.url_mount())
    {
        return respond_hotreload_js(request);
    }
    let mut response = match site {
        Some(site) => respond_published(request, site, reload_endpoint),
        None => respond_unpublished(
            request,
            &initial_config.url_mount(),
            reload_endpoint,
            unpublished_html,
        ),
    };
    if let Some(endpoint) = reload_endpoint {
        response.headers_mut().insert(
            "X-Tola-Reload-Generation",
            HeaderValue::from_str(endpoint.generation())
                .expect("reload generation is a header value"),
        );
    }
    response
}

fn respond_published(
    request: Request<()>,
    site: Arc<InstalledRevision>,
    reload_endpoint: Option<&ReloadEndpoint>,
) -> Response<ResponseBody> {
    let config = site.config().as_ref();
    if request
        .headers()
        .get("X-Tola-Hot-Reload")
        .and_then(|value| value.to_str().ok())
        == Some("true")
        && request
            .headers()
            .get("X-Tola-Revision")
            .and_then(|value| value.to_str().ok())
            != Some(site.manifest().revision().as_str())
    {
        return respond_revision_conflict(request.method());
    }
    let browser_path = match UrlPath::parse(request.uri().path()) {
        Ok(path) => path,
        Err(_) => {
            return text_status_response(
                request.method(),
                StatusCode::BAD_REQUEST,
                "400 Bad Request",
            );
        }
    };
    let Some(route) = config.url_mount().strip(&browser_path) else {
        return respond_not_found(request, config, site.outputs(), reload_endpoint, &site.html);
    };
    if let Some(output) = resolve_output(&route, site.outputs()) {
        return if output.path().as_str() == "404.html" && output.kind() == OutputKind::HtmlDocument
        {
            respond_not_found(request, config, site.outputs(), reload_endpoint, &site.html)
        } else {
            respond_file(
                request,
                output,
                &config.url_mount(),
                reload_endpoint,
                site.outputs(),
                &site.html,
            )
        };
    }
    if route.as_str() == "/"
        && site.page_availability() == tola_build::output::PageAvailability::Empty
    {
        let canonical = tola_build::build::no_pages_diagnostic(config);
        let diagnostic = site
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == tola_build::codes::site::NO_PAGES)
            .expect("empty published revisions hold the canonical no-pages diagnostic");
        debug_assert_eq!(diagnostic, &canonical);
        return respond_welcome(
            request,
            diagnostic,
            &config.url_mount(),
            reload_endpoint,
            site.manifest().revision(),
            site.page_availability(),
            &site.html,
        );
    }
    respond_not_found(request, config, site.outputs(), reload_endpoint, &site.html)
}

fn resolve_output<'a>(route: &UrlPath, revision: &'a OutputRevision) -> Option<&'a OutputFile> {
    revision.output(&tola_address::OutputPath::from_route(route))
}

fn respond_file(
    request: Request<()>,
    output: &OutputFile,
    url_mount: &SiteUrlMount,
    reload_endpoint: Option<&ReloadEndpoint>,
    revision: &OutputRevision,
    html: &HtmlResponses,
) -> Response<ResponseBody> {
    let content_type = output.declaration().media_type().as_str();
    if reload_endpoint.is_some() {
        match requested_representation(&request) {
            RequestedRepresentation::Absent => {}
            RequestedRepresentation::Duplicate => {
                return text_status_response(
                    request.method(),
                    StatusCode::BAD_REQUEST,
                    "400 Bad Request",
                );
            }
            RequestedRepresentation::Value(representation)
                if !revision
                    .manifest()
                    .matches_representation(output.path(), &representation) =>
            {
                return respond_revision_conflict(request.method());
            }
            RequestedRepresentation::Value(_) => {}
        }
    }
    let body = published_output_body(
        request.method(),
        output,
        url_mount,
        reload_endpoint,
        revision,
        html,
    );
    if request.method() != Method::HEAD
        // No validators are emitted, so an If-Range condition cannot match.
        && !request.headers().contains_key(IF_RANGE)
        && let Some(range) = request
            .headers()
            .get(RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|range| range.strip_prefix("bytes="))
    {
        return respond_range(body, content_type, range);
    }
    let mut response = respond_bytes(request.method(), StatusCode::OK, Some(content_type), body);
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
}

fn published_output_body(
    method: &Method,
    output: &OutputFile,
    url_mount: &SiteUrlMount,
    reload_endpoint: Option<&ReloadEndpoint>,
    revision: &OutputRevision,
    html: &HtmlResponses,
) -> ResponseBody {
    match reload_endpoint.filter(|_| output.kind() == OutputKind::HtmlDocument) {
        Some(endpoint) => html
            .page(Some(output.path()), url_mount, endpoint)
            .response(
                method,
                output.bytes().len(),
                || {
                    HtmlInjection::new(
                        url_mount,
                        endpoint,
                        Some(revision.manifest().revision()),
                        Some(revision.page_availability()),
                        Some(output.path()),
                    )
                },
                || Bytes::from_owner(output.bytes_owner()),
            ),
        None if method == Method::HEAD => ResponseBody::head_metadata(output.bytes().len()),
        None => ResponseBody::whole(Bytes::from_owner(output.bytes_owner())),
    }
}

enum RequestedRepresentation<'a> {
    Absent,
    Value(std::borrow::Cow<'a, str>),
    Duplicate,
}

fn requested_representation(request: &Request<()>) -> RequestedRepresentation<'_> {
    let Some(query) = request.uri().query() else {
        return RequestedRepresentation::Absent;
    };
    let mut representations = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(name, _)| name == "tola-representation")
        .map(|(_, value)| value);
    let Some(representation) = representations.next() else {
        return RequestedRepresentation::Absent;
    };
    if representations.next().is_some() {
        return RequestedRepresentation::Duplicate;
    }
    RequestedRepresentation::Value(representation)
}

fn respond_range(body: ResponseBody, content_type: &str, range: &str) -> Response<ResponseBody> {
    let file_size = body.byte_len() as u64;
    let Ok((start, end)) = parse_range(range, file_size) else {
        let mut response = respond_bytes(
            &Method::GET,
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
            ResponseBody::whole(Bytes::new()),
        );
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes */{file_size}"))
                .expect("byte range response is a valid header value"),
        );
        return response;
    };
    let partial = body
        .within(start as usize, end as usize)
        .expect("a satisfiable range names bytes this body has");
    let mut response = respond_bytes(
        &Method::GET,
        StatusCode::PARTIAL_CONTENT,
        Some(content_type),
        partial,
    );
    response.headers_mut().insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{end}/{file_size}"))
            .expect("byte range response is a valid header value"),
    );
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
}

fn parse_range(range: &str, file_size: u64) -> Result<(u64, u64)> {
    anyhow::ensure!(file_size > 0, "empty files have no satisfiable byte range");
    let range = range.trim();
    anyhow::ensure!(!range.contains(','), "multiple ranges are not supported");
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("the byte range is not a `start-end` range"))?;
    anyhow::ensure!(
        !end.contains('-'),
        "the byte range is not a `start-end` range"
    );
    let start = start.trim();
    let end = end.trim();
    anyhow::ensure!(!start.is_empty() || !end.is_empty(), "empty byte range");

    if start.is_empty() {
        let suffix: u64 = end.parse()?;
        anyhow::ensure!(suffix > 0, "suffix range must be positive");
        let length = suffix.min(file_size);
        return Ok((file_size - length, file_size - 1));
    }

    let start: u64 = start.parse()?;
    anyhow::ensure!(start < file_size, "range start exceeds file size");
    let end = if end.is_empty() {
        file_size - 1
    } else {
        end.parse::<u64>()?.min(file_size - 1)
    };
    anyhow::ensure!(end >= start, "range end precedes start");
    Ok((start, end))
}

fn respond_not_found(
    request: Request<()>,
    config: &ResolvedSiteConfig,
    revision: &OutputRevision,
    reload_endpoint: Option<&ReloadEndpoint>,
    html: &HtmlResponses,
) -> Response<ResponseBody> {
    let output = revision
        .output_by_path_text("404.html")
        .filter(|output| output.kind() == OutputKind::HtmlDocument);
    if let Some(output) = output {
        let body = published_output_body(
            request.method(),
            output,
            &config.url_mount(),
            reload_endpoint,
            revision,
            html,
        );
        return respond_bytes(
            request.method(),
            StatusCode::NOT_FOUND,
            Some(tola_build::output::semantics::ResponseMediaType::HTML.as_str()),
            body,
        );
    }
    text_status_response(request.method(), StatusCode::NOT_FOUND, "404 Not Found")
}

fn respond_revision_conflict(method: &Method) -> Response<ResponseBody> {
    text_status_response(
        method,
        StatusCode::CONFLICT,
        "the site changed; reload the page",
    )
}

fn text_status_response(
    method: &Method,
    status: StatusCode,
    message: &'static str,
) -> Response<ResponseBody> {
    respond_bytes(
        method,
        status,
        Some(tola_build::output::semantics::ResponseMediaType::PLAIN_TEXT.as_str()),
        ResponseBody::whole(Bytes::from_static(message.as_bytes())),
    )
}

fn respond_welcome(
    request: Request<()>,
    diagnostic: &Diagnostic,
    url_mount: &SiteUrlMount,
    reload_endpoint: Option<&ReloadEndpoint>,
    revision: &tola_build::output::manifest::RevisionId,
    page_availability: tola_build::output::PageAvailability,
    html: &HtmlResponses,
) -> Response<ResponseBody> {
    let body = html.welcome(|| {
    let message = tola_build::html::escape(&diagnostic.message);
    let help = diagnostic
        .help
        .first()
        .map(|help| tola_build::html::escape(&help.message));
    let help = help.map_or_else(String::new, |help| format!("<p>{help}</p>"));
    Bytes::from(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Tola</title></head><body><p>{message}</p>{help}</body></html>"
    ))
    });
    let body = match reload_endpoint {
        Some(endpoint) => html.page(None, url_mount, endpoint).response(
            request.method(),
            body.len(),
            || {
                HtmlInjection::new(
                    url_mount,
                    endpoint,
                    Some(revision),
                    Some(page_availability),
                    None,
                )
            },
            || body.clone(),
        ),
        None => ResponseBody::whole(body.clone()),
    };
    respond_bytes(
        request.method(),
        StatusCode::OK,
        Some(tola_build::output::semantics::ResponseMediaType::HTML.as_str()),
        body,
    )
}

fn respond_hotreload_js(request: Request<()>) -> Response<ResponseBody> {
    respond_development_runtime(request.method(), crate::embed::dev::hotreload_js())
}

/// The response for the development runtime, or the failure sentence when Tola could not prepare
/// it.
///
/// The failure itself is logged once, where the runtime is prepared; the runtime is never served
/// unminified.
fn respond_development_runtime(
    method: &Method,
    runtime: Result<&'static str, &crate::embed::dev::RuntimeMinifyError>,
) -> Response<ResponseBody> {
    match runtime {
        Ok(runtime) => respond_bytes(
            method,
            StatusCode::OK,
            Some(tola_build::output::semantics::ResponseMediaType::JAVASCRIPT.as_str()),
            ResponseBody::whole(Bytes::from_static(runtime.as_bytes())),
        ),
        Err(_) => text_status_response(
            method,
            StatusCode::INTERNAL_SERVER_ERROR,
            RUNTIME_UNAVAILABLE,
        ),
    }
}

/// What a site author reads when Tola cannot prepare its own development runtime.
const RUNTIME_UNAVAILABLE: &str = concat!(
    "Tola could not prepare its development runtime.\n\n",
    "This is a bug in Tola, not in your site. ",
    "Please report it at https://github.com/tola-rs/tola-ssg/issues.\n",
);

fn respond_unpublished(
    request: Request<()>,
    mount: &SiteUrlMount,
    endpoint: Option<&ReloadEndpoint>,
    html: &HtmlResponses,
) -> Response<ResponseBody> {
    let body = html.welcome(|| {
    let help = if endpoint.is_some() {
        "Fix the errors and save. This page will reload when the site builds successfully."
    } else {
        "Fix the errors, restart `tola dev`, then reload this page."
    };
    Bytes::from(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Build failed — Tola</title></head><body><main><h1>Build failed</h1><p>{help}</p></main></body></html>"
    ))
    });
    let body = match endpoint {
        Some(endpoint) => html.page(None, mount, endpoint).response(
            request.method(),
            body.len(),
            || HtmlInjection::new(mount, endpoint, None, None, None),
            || body.clone(),
        ),
        None => ResponseBody::whole(body.clone()),
    };
    respond_bytes(
        request.method(),
        StatusCode::SERVICE_UNAVAILABLE,
        Some(tola_build::output::semantics::ResponseMediaType::HTML.as_str()),
        body,
    )
}

fn respond_method_not_allowed(method: &Method) -> Response<ResponseBody> {
    let mut response = text_status_response(
        method,
        StatusCode::METHOD_NOT_ALLOWED,
        "405 Method Not Allowed",
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
    response
}

/// Every development response is fresh, including errors and sessions without reload.
/// HEAD retains the selected representation length while omitting its body.
pub(super) fn respond_bytes(
    method: &Method,
    status: StatusCode,
    content_type: Option<&str>,
    body: ResponseBody,
) -> Response<ResponseBody> {
    let content_length = body.byte_len() as u64;
    let mut response = Response::new(if method == Method::HEAD {
        ResponseBody::head_metadata(0)
    } else {
        body
    });
    *response.status_mut() = status;
    let headers = response.headers_mut();
    if let Some(content_type) = content_type {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(content_type)
                .expect("validated response media type is a header value"),
        );
    }
    headers.insert(CONTENT_LENGTH, HeaderValue::from(content_length));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate, max-age=0"),
    );
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(EXPIRES, HeaderValue::from_static("0"));
    response
}

#[cfg(test)]
mod tests {
    use super::super::tests::site_config;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use bytes::Bytes;
    use hyper::{Method, Request, Response, StatusCode};
    use tempfile::TempDir;
    use tola_address::{SiteUrlMount, UrlPath};
    use tola_build::output::revision::OutputRevision;
    use tola_build::output::semantics::ResponseMediaType;

    fn request(url: &str) -> Request<()> {
        request_with_method(Method::GET, url)
    }

    fn request_with_method(method: Method, url: &str) -> Request<()> {
        Request::builder().method(method).uri(url).body(()).unwrap()
    }

    fn request_with_header(
        method: Method,
        url: &str,
        name: &'static str,
        value: &str,
    ) -> Request<()> {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::HeaderName::from_static(name),
            hyper::header::HeaderValue::from_str(value).unwrap(),
        );
        let mut request = request_with_method(method, url);
        *request.headers_mut() = headers;
        request
    }

    fn reload_endpoint() -> crate::dev::reload::transport::ReloadEndpoint {
        crate::dev::reload::transport::ReloadEndpoint::new(35729, "test-session", "test-generation")
    }

    fn asset_revision(path: &str, bytes: &[u8]) -> tola_build::output::revision::OutputRevision {
        let extension = std::path::Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("bin");
        asset_revision_from_source(path, &format!("input.{extension}"), bytes)
    }

    fn html_revision(path: &str, body: &str) -> tola_build::output::revision::OutputRevision {
        let directory = tempfile::tempdir().unwrap();
        revision_from_site(
            directory.path(),
            &format!(
                "#document({})[{body}]",
                serde_json::to_string(path).unwrap()
            ),
            "",
        )
    }

    fn asset_revision_from_source(
        path: &str,
        source: &str,
        bytes: &[u8],
    ) -> tola_build::output::revision::OutputRevision {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(source), bytes).unwrap();
        let configuration = format!(
            "[build.minify]\ncss = false\njavascript = false\n[assets]\nfiles = [{{source = {}, url = {}}}]\n",
            serde_json::to_string(source).unwrap(),
            serde_json::to_string(&format!("/{path}")).unwrap()
        );
        revision_from_site(directory.path(), "", &configuration)
    }

    fn revision_from_site(
        root: &std::path::Path,
        program: &str,
        configuration: &str,
    ) -> tola_build::output::revision::OutputRevision {
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("site.typ"), program).unwrap();
        let config = site_config(root, configuration);
        let built =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Production)
                .unwrap();
        built.into_unchecked_revision(None).site().outputs().clone()
    }

    fn output<'a>(
        revision: &'a tola_build::output::revision::OutputRevision,
        path: &str,
    ) -> &'a tola_build::output::graph::OutputFile {
        revision
            .output(&tola_address::OutputPath::parse(path).unwrap())
            .unwrap()
    }

    #[test]
    fn satisfiable_ranges_resolve_to_bounds() {
        assert_eq!(super::parse_range("0-4", 10).unwrap(), (0, 4));
        assert_eq!(super::parse_range("7-", 10).unwrap(), (7, 9));
        assert_eq!(super::parse_range("-3", 10).unwrap(), (7, 9));
        assert_eq!(super::parse_range("-30", 10).unwrap(), (0, 9));
    }

    #[test]
    fn invalid_byte_ranges_are_rejected() {
        assert!(super::parse_range("0-", 0).is_err());
        assert!(super::parse_range("10-", 10).is_err());
        assert!(super::parse_range("7-2", 10).is_err());
        assert!(super::parse_range("-0", 10).is_err());
        assert!(super::parse_range("oops", 10).is_err());
        assert!(super::parse_range("0-1,3-4", 10).is_err());
    }

    #[test]
    fn unusable_ranges_serve_complete_body() {
        let revision = asset_revision("download.bin", b"0123456789");
        let mut conditional =
            request_with_header(Method::GET, "/download.bin", "range", "bytes=2-5");
        conditional.headers_mut().insert(
            hyper::header::IF_RANGE,
            hyper::header::HeaderValue::from_static("\"previous-revision\""),
        );

        for request in [
            conditional,
            request_with_header(Method::GET, "/download.bin", "range", "items=2-5"),
        ] {
            let response = super::respond_file(
                request,
                output(&revision, "download.bin"),
                &SiteUrlMount::root(),
                None,
                &revision,
                &super::HtmlResponses::default(),
            );

            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(body_bytes(response.body()).as_ref(), b"0123456789");
            assert!(
                !response
                    .headers()
                    .contains_key(hyper::header::CONTENT_RANGE)
            );
        }
    }

    #[test]
    fn error_responses_omit_head_body() {
        let get =
            super::text_status_response(&Method::GET, StatusCode::BAD_REQUEST, "400 Bad Request");
        assert_eq!(get.status().as_u16(), 400);
        assert_eq!(body_bytes(get.body()).as_ref(), b"400 Bad Request");

        let head =
            super::text_status_response(&Method::HEAD, StatusCode::BAD_REQUEST, "400 Bad Request");
        assert_eq!(head.status().as_u16(), 400);
        assert!(body_bytes(head.body()).is_empty());
    }

    #[test]
    fn stale_representation_is_rejected() {
        let revision = asset_revision("styles/site.css", b"body {}");
        let current = output(&revision, "styles/site.css");
        let representation =
            tola_build::output::manifest::RepresentationId::from_output(current).to_hex();
        let accepted = super::respond_file(
            request(&format!(
                "/styles/site.css?tola-representation={representation}"
            )),
            current,
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );
        assert_eq!(accepted.status().as_u16(), 200);
        assert_eq!(body_bytes(accepted.body()).as_ref(), b"body {}");

        let unknown = super::respond_file(
            request("/styles/site.css?tola-representation=stale"),
            current,
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );
        assert_eq!(unknown.status().as_u16(), 409);
        assert_eq!(
            body_bytes(unknown.body()).as_ref(),
            b"the site changed; reload the page"
        );

        let head = super::respond_file(
            request_with_method(Method::HEAD, "/styles/site.css?tola-representation=stale"),
            current,
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );
        assert_eq!(head.status().as_u16(), 409);
        assert!(body_bytes(head.body()).is_empty());

        let before = asset_revision_from_source("styles/site", "input.bin", b"body {}");
        let after = asset_revision_from_source("styles/site", "input.css", b"body {}");
        let previous = tola_build::output::manifest::RepresentationId::from_output(output(
            &before,
            "styles/site",
        ))
        .to_hex();
        let changed = super::respond_file(
            request(&format!("/styles/site?tola-representation={previous}")),
            output(&after, "styles/site"),
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &after,
            &super::HtmlResponses::default(),
        );
        assert_eq!(changed.status().as_u16(), 409);
        assert_eq!(
            body_bytes(changed.body()).as_ref(),
            b"the site changed; reload the page"
        );
    }

    #[test]
    fn user_representation_query_is_ignored() {
        let revision = asset_revision("styles/site.css", b"body {}");

        let response = super::respond_file(
            request("/styles/site.css?tola-representation=user-value"),
            output(&revision, "styles/site.css"),
            &SiteUrlMount::root(),
            None,
            &revision,
            &super::HtmlResponses::default(),
        );

        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(body_bytes(response.body()).as_ref(), b"body {}");
    }

    #[test]
    fn extensionless_asset_keeps_media_type() {
        let revision = asset_revision_from_source("styles/app", "input.css", b"body {}");
        let output = output(&revision, "styles/app");
        let css = ResponseMediaType::CSS;

        for response in [
            super::respond_file(
                request("/styles/app"),
                output,
                &SiteUrlMount::root(),
                None,
                &revision,
                &super::HtmlResponses::default(),
            ),
            super::respond_file(
                request_with_method(Method::HEAD, "/styles/app"),
                output,
                &SiteUrlMount::root(),
                None,
                &revision,
                &super::HtmlResponses::default(),
            ),
            super::respond_file(
                request_with_header(Method::GET, "/styles/app", "range", "bytes=0-3"),
                output,
                &SiteUrlMount::root(),
                None,
                &revision,
                &super::HtmlResponses::default(),
            ),
        ] {
            assert_eq!(
                response.headers()[hyper::header::CONTENT_TYPE],
                css.as_str()
            );
        }
    }

    #[test]
    fn html_without_reload_keeps_bytes() {
        let revision = html_revision("index.html", "exact");
        let response = super::respond_file(
            request("/index.html"),
            output(&revision, "index.html"),
            &SiteUrlMount::root(),
            None,
            &revision,
            &super::HtmlResponses::default(),
        );

        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            body_bytes(response.body()).as_ref(),
            output(&revision, "index.html").bytes()
        );
    }

    #[test]
    fn duplicate_representation_is_rejected() {
        let revision = asset_revision("site.css", b"body {}");
        let response = super::respond_file(
            request("/site.css?tola-representation=first&tola-representation=second"),
            output(&revision, "site.css"),
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );

        assert_eq!(response.status().as_u16(), 400);
        assert_eq!(body_bytes(response.body()).as_ref(), b"400 Bad Request");
    }

    #[test]
    fn asset_range_serves_selected_bytes() {
        let revision = asset_revision("media/video.mp4", b"0123456789");
        let output = output(&revision, "media/video.mp4");
        let response = super::respond_file(
            request_with_header(Method::GET, "/media/video.mp4", "range", "bytes=2-5"),
            output,
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );

        assert_eq!(response.status().as_u16(), 206);
        assert_eq!(body_bytes(response.body()).as_ref(), b"2345");
    }

    #[test]
    fn range_spans_injected_slices() {
        let revision = html_revision("index.html", "Home");
        let endpoint = reload_endpoint();
        let output = output(&revision, "index.html");
        let whole = super::respond_file(
            request("/"),
            output,
            &SiteUrlMount::root(),
            Some(&endpoint),
            &revision,
            &super::HtmlResponses::default(),
        );
        let partial = super::respond_file(
            request_with_header(Method::GET, "/", "range", "bytes=7-40"),
            output,
            &SiteUrlMount::root(),
            Some(&endpoint),
            &revision,
            &super::HtmlResponses::default(),
        );

        let whole = body_bytes(whole.body());
        assert_eq!(partial.status().as_u16(), 206);
        assert_eq!(body_bytes(partial.body()).as_ref(), &whole[7..41]);
    }

    fn has_no_store(response: &Response<super::ResponseBody>) -> bool {
        response
            .headers()
            .get(hyper::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("no-store"))
    }

    #[test]
    fn development_responses_are_never_stored() {
        let revision = asset_revision("site.css", b"body {}");
        let directory = tempfile::tempdir().unwrap();
        let config = site_config(directory.path(), "");
        let diagnostic = tola_build::build::no_pages_diagnostic(&config);
        let endpoint = reload_endpoint();
        let responses = [
            super::respond_file(
                request("/site.css"),
                output(&revision, "site.css"),
                &SiteUrlMount::root(),
                Some(&endpoint),
                &revision,
                &super::HtmlResponses::default(),
            ),
            super::respond_file(
                request_with_header(Method::GET, "/site.css", "range", "bytes=0-3"),
                output(&revision, "site.css"),
                &SiteUrlMount::root(),
                Some(&endpoint),
                &revision,
                &super::HtmlResponses::default(),
            ),
            super::respond_file(
                request_with_header(Method::GET, "/site.css", "range", "bytes=99-"),
                output(&revision, "site.css"),
                &SiteUrlMount::root(),
                Some(&endpoint),
                &revision,
                &super::HtmlResponses::default(),
            ),
            super::respond_not_found(
                request("/missing"),
                &config,
                &revision,
                Some(&endpoint),
                &super::HtmlResponses::default(),
            ),
            super::respond_welcome(
                request("/"),
                &diagnostic,
                &SiteUrlMount::root(),
                Some(&endpoint),
                revision.manifest().revision(),
                tola_build::output::PageAvailability::Empty,
                &super::HtmlResponses::default(),
            ),
            super::respond_hotreload_js(request("/hotreload.js")),
            super::respond(
                request("/"),
                None,
                &config,
                Some(&endpoint),
                &super::HtmlResponses::default(),
            ),
        ];

        for response in responses {
            assert!(has_no_store(&response), "{response:?}");
        }
    }

    fn content_length(response: Response<super::ResponseBody>) -> u64 {
        response.headers()["content-length"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// The bytes one development response has, joined from its slices.
    fn body_bytes(body: &super::ResponseBody) -> Bytes {
        body.remaining_slices()
            .iter()
            .flat_map(|slice| slice.iter().copied())
            .collect::<Vec<u8>>()
            .into()
    }

    #[test]
    fn head_reports_length_without_body() {
        let revision = asset_revision("media/video.mp4", b"0123456789");
        let response = super::respond_file(
            request_with_header(Method::HEAD, "/media/video.mp4", "range", "bytes=2-5"),
            output(&revision, "media/video.mp4"),
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );

        assert_eq!(response.status().as_u16(), 200);
        assert!(body_bytes(response.body()).is_empty());
        assert_eq!(content_length(response), 10);
    }

    #[test]
    fn method_not_allowed_declares_methods() {
        let directory = tempfile::tempdir().unwrap();
        let config = site_config(directory.path(), "");
        let response = super::respond(
            request_with_method(Method::POST, "/"),
            None,
            &config,
            None,
            &super::HtmlResponses::default(),
        );

        assert_eq!(response.status().as_u16(), 405);
        assert_eq!(
            body_bytes(response.body()).as_ref(),
            b"405 Method Not Allowed"
        );
        assert_eq!(response.headers()[hyper::header::ALLOW], "GET, HEAD");
    }

    #[test]
    fn injected_html_head_matches_get() {
        let revision = html_revision("index.html", "Page");
        let get = super::respond_file(
            request("/index.html"),
            output(&revision, "index.html"),
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );
        let head = super::respond_file(
            request_with_method(Method::HEAD, "/index.html"),
            output(&revision, "index.html"),
            &SiteUrlMount::root(),
            Some(&reload_endpoint()),
            &revision,
            &super::HtmlResponses::default(),
        );

        assert!(body_bytes(head.body()).is_empty());
        assert_eq!(content_length(head), body_bytes(get.body()).len() as u64);
    }

    #[test]
    fn unsatisfiable_range_reports_length() {
        let revision = asset_revision("media/video.mp4", b"0123456789");
        let response = super::respond_file(
            request_with_header(Method::GET, "/media/video.mp4", "range", "bytes=10-"),
            output(&revision, "media/video.mp4"),
            &SiteUrlMount::root(),
            None,
            &revision,
            &super::HtmlResponses::default(),
        );

        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response.headers()[hyper::header::CONTENT_RANGE],
            "bytes */10"
        );
        assert!(body_bytes(response.body()).is_empty());
        assert_eq!(content_length(response), 0);
    }

    #[test]
    fn reload_runtime_available_before_build() {
        let directory = tempfile::tempdir().unwrap();
        let config = site_config(directory.path(), "");
        let endpoint = reload_endpoint();
        let runtime = crate::embed::dev::hotreload_browser_path(&config.url_mount());
        let page = super::respond(
            request("/"),
            None,
            &config,
            Some(&endpoint),
            &super::HtmlResponses::default(),
        );
        let script = super::respond(
            request(&runtime),
            None,
            &config,
            Some(&endpoint),
            &super::HtmlResponses::default(),
        );
        let head = super::respond(
            request_with_method(Method::HEAD, &runtime),
            None,
            &config,
            Some(&endpoint),
            &super::HtmlResponses::default(),
        );

        assert_eq!(page.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(String::from_utf8_lossy(&body_bytes(page.body())).contains("data-tola-runtime"));
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(script.body()).as_ref(),
            crate::embed::dev::hotreload_js()
                .expect("Tola's own runtime sources minify")
                .as_bytes()
        );
        assert!(body_bytes(head.body()).is_empty());
        assert_eq!(content_length(head), body_bytes(script.body()).len() as u64);
    }

    #[test]
    fn missing_route_serves_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let revision = revision_from_site(directory.path(), "", "");
        let config = site_config(directory.path(), "");

        let get = super::respond_not_found(
            request("/missing"),
            &config,
            &revision,
            Some(&reload_endpoint()),
            &super::HtmlResponses::default(),
        );
        let head = super::respond_not_found(
            request_with_method(Method::HEAD, "/missing"),
            &config,
            &revision,
            Some(&reload_endpoint()),
            &super::HtmlResponses::default(),
        );

        assert_eq!(get.status().as_u16(), 404);
        assert_eq!(body_bytes(get.body()).as_ref(), b"404 Not Found");
        assert_eq!(head.status().as_u16(), 404);
        assert!(body_bytes(head.body()).is_empty());
    }

    #[test]
    fn empty_site_root_serves_welcome() {
        for (configuration, root_path) in [("", "/"), ("[site]\nbase-path = \"/docs/\"", "/docs/")]
        {
            let directory = TempDir::new().unwrap();
            let site = published_site(directory.path(), "", configuration);
            let diagnostic = tola_build::build::no_pages_diagnostic(site.config());
            let endpoint = reload_endpoint();
            let get =
                super::respond_published(request(root_path), Arc::clone(&site), Some(&endpoint));
            let head = super::respond_published(
                request_with_method(Method::HEAD, root_path),
                Arc::clone(&site),
                Some(&endpoint),
            );

            assert_eq!(get.status(), StatusCode::OK);
            assert_eq!(
                get.headers()[hyper::header::CONTENT_TYPE],
                ResponseMediaType::HTML.as_str()
            );
            let body = body_bytes(get.body());
            let html = String::from_utf8_lossy(&body);
            assert!(html.contains(tola_build::html::escape(&diagnostic.message).as_ref()));
            assert!(html.contains(tola_build::html::escape(&diagnostic.help[0].message).as_ref()));
            assert!(html.contains("data-tola-runtime"));
            assert_eq!(head.status(), StatusCode::OK);
            assert_eq!(
                head.headers()[hyper::header::CONTENT_TYPE],
                get.headers()[hyper::header::CONTENT_TYPE]
            );
            assert!(body_bytes(head.body()).is_empty());
            assert_eq!(content_length(head), body.len() as u64);
        }
    }

    #[test]
    fn hot_reload_head_omits_body() {
        let get = super::respond_hotreload_js(request("/hotreload.js"));
        let head = super::respond_hotreload_js(request_with_method(Method::HEAD, "/hotreload.js"));

        assert_eq!(get.status().as_u16(), 200);
        assert_eq!(
            body_bytes(get.body()).as_ref(),
            crate::embed::dev::hotreload_js()
                .expect("Tola's own runtime sources minify")
                .as_bytes()
        );
        assert_eq!(head.status().as_u16(), 200);
        assert!(body_bytes(head.body()).is_empty());
    }

    #[test]
    fn hotreload_response_serves_minified_runtime() {
        let response = super::respond_hotreload_js(request("/hotreload.js"));
        let body = String::from_utf8_lossy(&body_bytes(response.body())).into_owned();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !body.contains("Development runtime for pages served"),
            "the served runtime is minified"
        );
        assert!(
            !body.contains("#tola-dev-status {"),
            "the status stylesheet is minified"
        );
        assert!(
            !body.contains("__TOLA_DEV_STATUS_CSS__"),
            "the status stylesheet placeholder is replaced"
        );
    }

    #[test]
    fn runtime_preparation_failure_is_reported() {
        let failure =
            crate::embed::dev::RuntimeMinifyError::RuntimeSource("deliberate failure".into());
        let response = super::respond_development_runtime(&Method::GET, Err(&failure));

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = String::from_utf8_lossy(&body_bytes(response.body())).into_owned();
        assert!(
            body.contains("could not prepare its development runtime"),
            "{body}"
        );
        assert!(
            body.contains("https://github.com/tola-rs/tola-ssg/issues"),
            "{body}"
        );
        assert!(
            !body.contains("deliberate failure"),
            "the minifier's own message stays in the log"
        );
    }

    fn published_site(
        root: &Path,
        program: &str,
        configuration: &str,
    ) -> Arc<super::InstalledRevision> {
        fs::create_dir_all(root.join("content")).unwrap();
        fs::write(root.join("site.typ"), program).unwrap();
        let config = site_config(root, configuration);
        let mut session = tola_build::build::BuildSession::new();
        let attempt = session.prepare(
            Arc::new(config),
            tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Production),
        );
        let checked = attempt
            .run()
            .unwrap()
            .into_unchecked_revision(None)
            .check(&tola_build::cancellation::BuildCancellation::new())
            .unwrap()
            .into_checked()
            .unwrap();
        Arc::new(super::InstalledRevision::new(Arc::new(
            session.install_revision(checked, Ok).unwrap(),
        )))
    }

    fn published_document(root: &Path, path: &str, body: &str) -> Arc<super::InstalledRevision> {
        published_site(
            root,
            &format!(
                "#document({})[{body}]",
                serde_json::to_string(path).unwrap()
            ),
            "",
        )
    }

    #[test]
    fn directory_file_and_missing_routes_serve_exact() {
        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("download.txt"), "download").unwrap();
        let site = published_site(
            directory.path(),
            "#document(\"guide/index.html\")[Guide]\n#document(\"404.html\")[Missing]",
            "[assets]\nfiles = [{source = \"download.txt\", url = \"/download\"}]",
        );

        // `guide` without its slash names the file `guide`, which this revision never published;
        // only the trailing slash names the directory index.
        for (path, status, output_path) in [
            ("/guide/", StatusCode::OK, "guide/index.html"),
            ("/guide/index.html?x=1", StatusCode::OK, "guide/index.html"),
            ("/guide", StatusCode::NOT_FOUND, "404.html"),
            ("/download", StatusCode::OK, "download"),
            ("/missing", StatusCode::NOT_FOUND, "404.html"),
            ("/404.html", StatusCode::NOT_FOUND, "404.html"),
        ] {
            for method in [Method::GET, Method::HEAD] {
                let response = super::respond_published(
                    request_with_method(method.clone(), path),
                    Arc::clone(&site),
                    None,
                );
                assert_eq!(response.status(), status, "{method} {path}");
                assert!(!response.headers().contains_key(hyper::header::LOCATION));
                if method == Method::HEAD {
                    assert!(body_bytes(response.body()).is_empty());
                } else {
                    assert_eq!(
                        body_bytes(response.body()).as_ref(),
                        output(site.outputs(), output_path).bytes()
                    );
                }
            }
        }
    }

    #[test]
    fn escaped_mounted_route_resolves_its_output() {
        let directory = TempDir::new().unwrap();
        let site = published_site(
            directory.path(),
            "#document(\"index.html\")[Home]\n#document(\"release..notes/中文/index.html\")[Guide]",
            "[site]\nbase-path = \"/文档/\"",
        );

        let response = super::respond_published(
            request("/%e6%96%87%e6%a1%a3/release..notes/%e4%b8%ad%e6%96%87/"),
            Arc::clone(&site),
            None,
        );

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(response.body()).as_ref(),
            output(site.outputs(), "release..notes/中文/index.html").bytes()
        );
    }

    fn hot_reload_request_with_method(method: hyper::Method, revision: &str) -> Request<()> {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::HeaderName::from_static("x-tola-hot-reload"),
            hyper::header::HeaderValue::from_static("true"),
        );
        headers.insert(
            hyper::header::HeaderName::from_static("x-tola-revision"),
            hyper::header::HeaderValue::from_str(revision).unwrap(),
        );
        let mut request = Request::builder().method(method).uri("/").body(()).unwrap();
        *request.headers_mut() = headers;
        request
    }

    #[test]
    fn generation_header_is_public_identity() {
        let directory = TempDir::new().unwrap();
        let site = published_document(directory.path(), "index.html", "unchanged");
        let config = site.config();
        let first = super::ReloadEndpoint::new(35729, "secret-first", "public-first");
        let restarted = super::ReloadEndpoint::new(35729, "secret-next", "public-next");
        let original = super::respond(
            request_with_method(Method::HEAD, "/"),
            Some(Arc::clone(&site)),
            config,
            Some(&first),
            &super::HtmlResponses::default(),
        );
        let next = super::respond(
            request_with_method(Method::HEAD, "/"),
            Some(Arc::clone(&site)),
            config,
            Some(&restarted),
            &super::HtmlResponses::default(),
        );
        assert_eq!(original.status(), StatusCode::OK);
        assert_eq!(next.status(), StatusCode::OK);
        assert_ne!(
            original.headers()["X-Tola-Reload-Generation"],
            next.headers()["X-Tola-Reload-Generation"]
        );
        assert!(body_bytes(next.body()).is_empty());
        assert!(next.headers().values().all(|value| {
            !value
                .as_bytes()
                .windows(b"secret-".len())
                .any(|part| part == b"secret-")
        }));

        // Every response has the restart identity, including a failed build, a removed
        // page, and a fenced hot-reload revision.
        let unavailable = super::respond(
            request_with_method(Method::HEAD, "/"),
            None,
            config,
            Some(&restarted),
            &super::HtmlResponses::default(),
        );
        let missing = super::respond(
            request_with_method(Method::HEAD, "/removed"),
            Some(Arc::clone(&site)),
            config,
            Some(&restarted),
            &super::HtmlResponses::default(),
        );
        let fenced = super::respond(
            hot_reload_request_with_method(Method::HEAD, "stale"),
            Some(Arc::clone(&site)),
            config,
            Some(&restarted),
            &super::HtmlResponses::default(),
        );
        let fenced_get = super::respond(
            hot_reload_request_with_method(Method::GET, "stale"),
            Some(Arc::clone(&site)),
            config,
            Some(&restarted),
            &super::HtmlResponses::default(),
        );
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(fenced.status(), StatusCode::CONFLICT);
        assert_eq!(fenced_get.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_bytes(fenced_get.body()).as_ref(),
            b"the site changed; reload the page"
        );
        for response in [unavailable, missing, fenced] {
            assert_eq!(
                response.headers()["X-Tola-Reload-Generation"],
                next.headers()["X-Tola-Reload-Generation"]
            );
            assert!(body_bytes(response.body()).is_empty());
        }
    }

    #[test]
    fn missing_routes_keep_fallback_status() {
        for (configuration, missing_paths) in [
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
            let directory = TempDir::new().unwrap();
            let site = published_site(
                directory.path(),
                "#document(\"404.html\")[Not Found]",
                configuration,
            );
            for path in missing_paths {
                let get = super::respond_published(request(path), Arc::clone(&site), None);
                let head = super::respond_published(
                    request_with_method(Method::HEAD, path),
                    Arc::clone(&site),
                    None,
                );

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
    fn asset_404_url_is_ordinary() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("not-found.html"), "ordinary asset").unwrap();
        let site = published_site(
            dir.path(),
            "",
            "[assets]\nfiles = [{source = \"not-found.html\", url = \"/404.html\"}]",
        );
        let request = request("/404.html");

        let response = super::respond_published(request, site, Some(&reload_endpoint()));

        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(body_bytes(response.body()).as_ref(), b"ordinary asset");
    }

    #[test]
    fn generated_site_serves_not_found() {
        let dir = TempDir::new().unwrap();
        let site = published_document(dir.path(), "generated.html", "generated");
        let request = request("/missing");

        let response = super::respond_published(request, site, Some(&reload_endpoint()));

        assert_eq!(response.status().as_u16(), 404);
        assert_eq!(body_bytes(response.body()).as_ref(), b"404 Not Found");
    }

    #[test]
    fn non_html_sites_preserve_route_status() {
        for program in ["", "#document(\"download.pdf\", format: \"pdf\")[Download]"] {
            let directory = TempDir::new().unwrap();
            fs::write(directory.path().join("theme.css"), "body {}").unwrap();
            let site = published_site(
                directory.path(),
                program,
                "[build.minify]\ncss = false\n[assets]\nfiles = [{ source = \"theme.css\", url = \"/theme.css\" }]",
            );
            for (path, status) in [
                ("/", StatusCode::OK),
                ("/theme.css", StatusCode::OK),
                ("/missing", StatusCode::NOT_FOUND),
            ] {
                let get = super::respond_published(request(path), Arc::clone(&site), None);
                let head = super::respond_published(
                    request_with_method(Method::HEAD, path),
                    Arc::clone(&site),
                    None,
                );

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

    fn revision<const N: usize>(outputs: [(&'static str, &'static [u8]); N]) -> OutputRevision {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(directory.path().join("site.typ"), "").unwrap();
        let mut configuration = String::from("[assets]\nfiles = [\n");
        for (index, (path, bytes)) in outputs.into_iter().enumerate() {
            let extension = Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("bin");
            let source = format!("input-{index}.{extension}");
            std::fs::write(directory.path().join(&source), bytes).unwrap();
            configuration.push_str(&format!(
                "{{source = {}, url = {}}},\n",
                serde_json::to_string(&source).unwrap(),
                serde_json::to_string(&format!("/{path}")).unwrap()
            ));
        }
        configuration.push_str("]\n");
        let config = site_config(directory.path(), &configuration);
        let built =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Production)
                .unwrap();
        built.into_unchecked_revision(None).site().outputs().clone()
    }

    #[test]
    fn encoded_dot_segments_resolve() {
        let revision = revision([("release..notes/\u{4e2d}\u{6587}/index.html", b"ok")]);

        let url = UrlPath::parse("/release..notes/%E4%B8%AD%E6%96%87/").unwrap();
        let resolved = super::resolve_output(&url, &revision).unwrap();
        assert_eq!(
            resolved.path().as_str(),
            "release..notes/\u{4e2d}\u{6587}/index.html"
        );
        assert_eq!(resolved.bytes(), b"ok");
    }

    #[test]
    fn revision_membership_decides_resolution() {
        let revision = revision([("posts/rust/index.html", b"post"), ("feed.xml", b"feed")]);

        let post = UrlPath::parse("/posts/rust/").unwrap();
        let feed = UrlPath::parse("/feed.xml").unwrap();
        let missing = UrlPath::parse("/outside/").unwrap();

        assert_eq!(
            super::resolve_output(&post, &revision).unwrap().bytes(),
            b"post"
        );
        assert_eq!(
            super::resolve_output(&feed, &revision).unwrap().bytes(),
            b"feed"
        );
        assert!(super::resolve_output(&missing, &revision).is_none());
    }
}
