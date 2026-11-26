//! Routing and HTTP representations of one published development revision.

use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use hyper::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, EXPIRES,
    HeaderValue, IF_RANGE, PRAGMA, RANGE,
};
use hyper::{Method, Request, Response, StatusCode};
use tola_address::{OutputPath, SiteUrlMount, UrlPath};
use tola_build::config::SiteConfig;
use tola_build::output::PageAvailability;
use tola_build::output::graph::{OutputFile, OutputKind};
use tola_build::output::manifest::RevisionId;
use tola_build::output::revision::OutputRevision;
use tola_build::output::semantics::ResponseMediaType;
use tola_diagnostic::Diagnostic;

use super::html::HtmlInjection;
use crate::dev::reload::transport::ReloadEndpoint;
use crate::dev::site::{HtmlResponses, InstalledRevision};

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

    #[cfg(test)]
    pub(super) fn remaining_slices(&self) -> &[Bytes] {
        self.slice_window()
    }

    fn slice_window(&self) -> &[Bytes] {
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
        for slice in self.slice_window() {
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
        _cx: &mut std::task::Context<'_>,
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

fn header_str<'a>(req: &'a Request<()>, name: &str) -> Option<&'a str> {
    req.headers().get(name).and_then(|val| val.to_str().ok())
}

pub(super) fn respond(
    req: Request<()>,
    site: Option<Arc<InstalledRevision>>,
    initial_cfg: &SiteConfig,
    reload_endpoint: Option<&ReloadEndpoint>,
    unpublished_html: &HtmlResponses,
) -> Response<ResponseBody> {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return respond_method_not_allowed(req.method());
    }
    let cfg = site
        .as_ref()
        .map_or(initial_cfg, |site| site.config().as_ref());
    if reload_endpoint.is_some()
        && req.uri().path() == crate::embed::dev::hotreload_browser_path(cfg.url_mount())
    {
        return respond_hotreload_js(req);
    }
    let mut resp = match site.as_deref() {
        Some(site) => respond_published(req, site, reload_endpoint),
        None => respond_unpublished(
            req,
            initial_cfg.url_mount(),
            reload_endpoint,
            unpublished_html,
        ),
    };
    if let Some(endpoint) = reload_endpoint {
        resp.headers_mut().insert(
            "X-Tola-Reload-Generation",
            HeaderValue::from_str(endpoint.generation())
                .expect("reload generation is a header value"),
        );
    }
    resp
}

fn respond_published(
    req: Request<()>,
    site: &InstalledRevision,
    reload_endpoint: Option<&ReloadEndpoint>,
) -> Response<ResponseBody> {
    let cfg = site.config();
    let mount = cfg.url_mount();
    let rev = site.outputs();
    if header_str(&req, "X-Tola-Hot-Reload") == Some("true")
        && header_str(&req, "X-Tola-Revision") != Some(site.manifest().revision().as_str())
    {
        return respond_revision_conflict(req.method());
    }
    let Ok(browser_path) = UrlPath::parse(req.uri().path()) else {
        return text_status_response(req.method(), StatusCode::BAD_REQUEST, "400 Bad Request");
    };
    let Some(route) = mount.strip(&browser_path) else {
        return respond_not_found(req, mount, rev, reload_endpoint, &site.html);
    };
    if let Some(output) = rev.output(&OutputPath::from_route(&route)) {
        return if output.path().as_str() == "404.html" && output.kind() == OutputKind::HtmlDocument
        {
            respond_not_found(req, mount, rev, reload_endpoint, &site.html)
        } else {
            respond_file(req, output, mount, reload_endpoint, rev, &site.html)
        };
    }
    if route.as_str() == "/" && site.page_availability() == PageAvailability::Empty {
        let canonical = tola_build::build::no_pages_diagnostic(cfg);
        let diag = site
            .diagnostics()
            .iter()
            .find(|diag| diag.code == tola_diagnostic::codes::site::NO_PAGES)
            .expect("empty published revisions hold the canonical no-pages diagnostic");
        debug_assert_eq!(diag, &canonical);
        return respond_welcome(
            req,
            diag,
            mount,
            reload_endpoint,
            site.manifest().revision(),
            site.page_availability(),
            &site.html,
        );
    }
    respond_not_found(req, mount, rev, reload_endpoint, &site.html)
}

fn respond_file(
    req: Request<()>,
    output: &OutputFile,
    mount: &SiteUrlMount,
    reload_endpoint: Option<&ReloadEndpoint>,
    rev: &OutputRevision,
    html: &HtmlResponses,
) -> Response<ResponseBody> {
    let content_type = output.declaration().media_type().as_str();
    if reload_endpoint.is_some() {
        match requested_representation(&req) {
            RequestedRepresentation::Absent => {}
            RequestedRepresentation::Duplicate => {
                return text_status_response(
                    req.method(),
                    StatusCode::BAD_REQUEST,
                    "400 Bad Request",
                );
            }
            RequestedRepresentation::Value(repr)
                if !rev.manifest().matches_representation(output.path(), &repr) =>
            {
                return respond_revision_conflict(req.method());
            }
            RequestedRepresentation::Value(_) => {}
        }
    }
    let body = published_output_body(req.method(), output, mount, reload_endpoint, rev, html);
    if req.method() != Method::HEAD
        // No validators are emitted, so an If-Range condition cannot match.
        && !req.headers().contains_key(IF_RANGE)
        && let Some(range) = req
            .headers()
            .get(RANGE)
            .and_then(|val| val.to_str().ok())
            .and_then(|range| range.strip_prefix("bytes="))
    {
        return respond_range(body, content_type, range);
    }
    let mut resp = respond_bytes(req.method(), StatusCode::OK, Some(content_type), body);
    resp.headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    resp
}

fn published_output_body(
    method: &Method,
    output: &OutputFile,
    mount: &SiteUrlMount,
    reload_endpoint: Option<&ReloadEndpoint>,
    rev: &OutputRevision,
    html: &HtmlResponses,
) -> ResponseBody {
    match reload_endpoint.filter(|_| output.kind() == OutputKind::HtmlDocument) {
        Some(endpoint) => html.page(Some(output.path())).response(
            method,
            output.bytes().len(),
            || {
                HtmlInjection::new(
                    mount,
                    endpoint,
                    Some(rev.manifest().revision()),
                    Some(rev.page_availability()),
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

fn requested_representation(req: &Request<()>) -> RequestedRepresentation<'_> {
    let Some(query) = req.uri().query() else {
        return RequestedRepresentation::Absent;
    };
    let mut reprs = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(name, _)| name == "tola-representation")
        .map(|(_, val)| val);
    let Some(repr) = reprs.next() else {
        return RequestedRepresentation::Absent;
    };
    if reprs.next().is_some() {
        return RequestedRepresentation::Duplicate;
    }
    RequestedRepresentation::Value(repr)
}

fn respond_range(body: ResponseBody, content_type: &str, range: &str) -> Response<ResponseBody> {
    let file_size = body.byte_len() as u64;
    let Some((start, end)) = parse_range(range, file_size) else {
        let mut resp = respond_bytes(
            &Method::GET,
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
            ResponseBody::whole(Bytes::new()),
        );
        resp.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes */{file_size}"))
                .expect("byte range response is a valid header value"),
        );
        return resp;
    };
    let partial = body
        .within(start as usize, end as usize)
        .expect("a satisfiable range names bytes this body has");
    let mut resp = respond_bytes(
        &Method::GET,
        StatusCode::PARTIAL_CONTENT,
        Some(content_type),
        partial,
    );
    resp.headers_mut().insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{end}/{file_size}"))
            .expect("byte range response is a valid header value"),
    );
    resp.headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    resp
}

fn parse_range(range: &str, file_size: u64) -> Option<(u64, u64)> {
    let last = file_size.checked_sub(1)?;
    let (start, end) = range.split_once('-')?;
    let start = start.trim();
    let end = end.trim();

    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        return (suffix > 0).then_some((file_size.saturating_sub(suffix), last));
    }

    let start: u64 = start.parse().ok()?;
    let end = if end.is_empty() {
        last
    } else {
        end.parse::<u64>().ok()?.min(last)
    };
    (start <= end).then_some((start, end))
}

fn respond_not_found(
    req: Request<()>,
    mount: &SiteUrlMount,
    rev: &OutputRevision,
    reload_endpoint: Option<&ReloadEndpoint>,
    html: &HtmlResponses,
) -> Response<ResponseBody> {
    let output = rev
        .output_by_path_text("404.html")
        .filter(|output| output.kind() == OutputKind::HtmlDocument);
    if let Some(output) = output {
        let body = published_output_body(req.method(), output, mount, reload_endpoint, rev, html);
        return respond_bytes(
            req.method(),
            StatusCode::NOT_FOUND,
            Some(ResponseMediaType::HTML.as_str()),
            body,
        );
    }
    text_status_response(req.method(), StatusCode::NOT_FOUND, "404 Not Found")
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
    msg: &'static str,
) -> Response<ResponseBody> {
    respond_bytes(
        method,
        status,
        Some(ResponseMediaType::PLAIN_TEXT.as_str()),
        ResponseBody::whole(Bytes::from_static(msg.as_bytes())),
    )
}

fn respond_welcome(
    req: Request<()>,
    diag: &Diagnostic,
    mount: &SiteUrlMount,
    reload_endpoint: Option<&ReloadEndpoint>,
    rev: &RevisionId,
    page_avail: PageAvailability,
    html: &HtmlResponses,
) -> Response<ResponseBody> {
    let body = html.welcome(|| {
        let msg = tola_build::html::escape(&diag.message);
        let help = diag
            .help
            .first()
            .map(|help| tola_build::html::escape(&help.message))
            .map_or_else(String::new, |help| format!("<p>{help}</p>"));
        Bytes::from(format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Tola</title></head><body><p>{msg}</p>{help}</body></html>"
        ))
    });
    respond_generated_html(&req, StatusCode::OK, html, body.clone(), || {
        reload_endpoint
            .map(|endpoint| HtmlInjection::new(mount, endpoint, Some(rev), Some(page_avail), None))
    })
}

fn respond_hotreload_js(req: Request<()>) -> Response<ResponseBody> {
    respond_development_runtime(req.method(), crate::embed::dev::hotreload_js())
}

fn respond_development_runtime(
    method: &Method,
    runtime: Result<&'static str, &crate::embed::dev::RuntimeMinifyError>,
) -> Response<ResponseBody> {
    match runtime {
        Ok(runtime) => respond_bytes(
            method,
            StatusCode::OK,
            Some(ResponseMediaType::JAVASCRIPT.as_str()),
            ResponseBody::whole(Bytes::from_static(runtime.as_bytes())),
        ),
        Err(_) => text_status_response(
            method,
            StatusCode::INTERNAL_SERVER_ERROR,
            RUNTIME_UNAVAILABLE,
        ),
    }
}

const RUNTIME_UNAVAILABLE: &str = concat!(
    "Tola could not prepare its development runtime.\n\n",
    "This is a bug in Tola, not in your site. ",
    "Please report it at https://github.com/tola-rs/tola-ssg/issues.\n",
);

fn respond_unpublished(
    req: Request<()>,
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
    respond_generated_html(
        &req,
        StatusCode::SERVICE_UNAVAILABLE,
        html,
        body.clone(),
        || endpoint.map(|endpoint| HtmlInjection::new(mount, endpoint, None, None, None)),
    )
}

fn respond_generated_html(
    req: &Request<()>,
    status: StatusCode,
    html: &HtmlResponses,
    body: Bytes,
    injection: impl FnOnce() -> Option<HtmlInjection>,
) -> Response<ResponseBody> {
    let body = match injection() {
        Some(injection) => {
            html.page(None)
                .response(req.method(), body.len(), || injection, || body.clone())
        }
        None => ResponseBody::whole(body),
    };
    respond_bytes(
        req.method(),
        status,
        Some(ResponseMediaType::HTML.as_str()),
        body,
    )
}

fn respond_method_not_allowed(method: &Method) -> Response<ResponseBody> {
    let mut resp = text_status_response(
        method,
        StatusCode::METHOD_NOT_ALLOWED,
        "405 Method Not Allowed",
    );
    resp.headers_mut()
        .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
    resp
}

/// Every development response disables caching; HEAD reports length without body bytes.
pub(super) fn respond_bytes(
    method: &Method,
    status: StatusCode,
    content_type: Option<&str>,
    body: ResponseBody,
) -> Response<ResponseBody> {
    let content_length = body.byte_len() as u64;
    let mut resp = Response::new(if method == Method::HEAD {
        ResponseBody::head_metadata(0)
    } else {
        body
    });
    *resp.status_mut() = status;
    let hdrs = resp.headers_mut();
    if let Some(content_type) = content_type {
        hdrs.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(content_type)
                .expect("validated response media type is a header value"),
        );
    }
    hdrs.insert(CONTENT_LENGTH, HeaderValue::from(content_length));
    hdrs.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate, max-age=0"),
    );
    hdrs.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    hdrs.insert(EXPIRES, HeaderValue::from_static("0"));
    resp
}

#[cfg(test)]
#[path = "response_tests.rs"]
mod tests;
