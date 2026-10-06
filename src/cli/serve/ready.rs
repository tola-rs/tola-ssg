use crate::config::SiteConfig;
use percent_encoding::percent_decode_str;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub(crate) struct ServeReady {
    init: AtomicBool,
    scan: AtomicBool,
    startup_done: AtomicBool,
    last_request_ms: AtomicU64,
}

impl ServeReady {
    pub(crate) fn new() -> Self {
        Self {
            init: AtomicBool::new(false),
            scan: AtomicBool::new(false),
            startup_done: AtomicBool::new(false),
            last_request_ms: AtomicU64::new(0),
        }
    }

    pub(crate) fn reset_startup(&self) {
        self.init.store(false, Ordering::SeqCst);
        self.scan.store(false, Ordering::SeqCst);
        self.startup_done.store(false, Ordering::SeqCst);
        self.note_request();
    }

    pub(crate) fn set_init_ready(&self) {
        self.init.store(true, Ordering::SeqCst);
    }

    pub(crate) fn init_ready(&self) -> bool {
        self.init.load(Ordering::SeqCst)
    }

    pub(crate) fn set_scan_ready(&self, ready: bool) {
        self.scan.store(ready, Ordering::SeqCst);
    }

    pub(crate) fn scan_ready(&self) -> bool {
        self.scan.load(Ordering::SeqCst)
    }

    pub(crate) fn set_startup_done(&self) {
        self.startup_done.store(true, Ordering::SeqCst);
    }

    pub(crate) fn startup_done(&self) -> bool {
        self.startup_done.load(Ordering::SeqCst)
    }

    pub(crate) fn note_request(&self) {
        self.last_request_ms.store(now_millis(), Ordering::SeqCst);
    }

    pub(crate) fn request_idle_for(&self, duration: Duration) -> bool {
        let last = self.last_request_ms.load(Ordering::SeqCst);
        last == 0 || now_millis().saturating_sub(last) >= duration.as_millis() as u64
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn page_ready(
    path: &Path,
    request_url: &str,
    config: &SiteConfig,
    ready: &ServeReady,
) -> bool {
    match has_missing_blocking_head_file(path, request_url, config) {
        Ok(false) => true,
        Ok(true) | Err(_) => ready.startup_done(),
    }
}

fn has_missing_blocking_head_file(
    path: &Path,
    request_url: &str,
    config: &SiteConfig,
) -> std::io::Result<bool> {
    let html = std::fs::read_to_string(path)?;
    Ok(
        blocking_head_files(&html, request_url, &config.build.output)
            .into_iter()
            .any(|path| !path.is_file()),
    )
}

fn blocking_head_files(html: &str, request_url: &str, output_root: &Path) -> Vec<PathBuf> {
    let Ok(dom) = tl::parse(html, tl::ParserOptions::default()) else {
        return Vec::new();
    };

    let Some(heads) = dom.query_selector("head") else {
        return Vec::new();
    };

    let mut resources = Vec::new();
    let parser = dom.parser();
    for handle in heads {
        let Some(node) = handle.get(parser) else {
            continue;
        };
        let Some(head) = node.as_tag() else {
            continue;
        };
        collect_head_files(head, parser, request_url, output_root, &mut resources);
    }

    resources.sort();
    resources.dedup();
    resources
}

fn collect_head_files(
    tag: &tl::HTMLTag<'_>,
    parser: &tl::Parser<'_>,
    request_url: &str,
    output_root: &Path,
    resources: &mut Vec<PathBuf>,
) {
    for child in tag.children().top().iter() {
        collect_head_node(*child, parser, request_url, output_root, resources);
    }
}

fn collect_head_node(
    handle: tl::NodeHandle,
    parser: &tl::Parser<'_>,
    request_url: &str,
    output_root: &Path,
    resources: &mut Vec<PathBuf>,
) {
    let Some(node) = handle.get(parser) else {
        return;
    };
    let Some(tag) = node.as_tag() else {
        return;
    };

    if let Some(href) = blocking_stylesheet_href(tag)
        && let Some(path) = resource_output_path(&href, request_url, output_root)
    {
        resources.push(path);
    }

    if let Some(src) = blocking_script_src(tag)
        && let Some(path) = resource_output_path(&src, request_url, output_root)
    {
        resources.push(path);
    }

    collect_head_files(tag, parser, request_url, output_root, resources);
}

fn blocking_stylesheet_href(tag: &tl::HTMLTag<'_>) -> Option<String> {
    if !tag.name().as_utf8_str().eq_ignore_ascii_case("link") {
        return None;
    }
    if has_attr(tag, "disabled") {
        return None;
    }

    rel_includes(tag, "stylesheet")
        .then(|| attr_value(tag, "href"))
        .flatten()
}

fn blocking_script_src(tag: &tl::HTMLTag<'_>) -> Option<String> {
    if !tag.name().as_utf8_str().eq_ignore_ascii_case("script") {
        return None;
    }

    if has_attr(tag, "async") || has_attr(tag, "defer") {
        return None;
    }

    let script_type = attr_value(tag, "type").unwrap_or_default();
    if !is_classic_script_type(&script_type) {
        return None;
    }

    attr_value(tag, "src")
}

fn rel_includes(tag: &tl::HTMLTag<'_>, token: &str) -> bool {
    attr_value(tag, "rel").is_some_and(|rel| {
        rel.split_ascii_whitespace()
            .any(|value| value.eq_ignore_ascii_case(token))
    })
}

fn attr_value(tag: &tl::HTMLTag<'_>, name: &str) -> Option<String> {
    tag.attributes().iter().find_map(|(attr, value)| {
        attr.eq_ignore_ascii_case(name)
            .then(|| value.as_ref().map(ToString::to_string))?
    })
}

fn has_attr(tag: &tl::HTMLTag<'_>, name: &str) -> bool {
    tag.attributes()
        .iter()
        .any(|(attr, _)| attr.eq_ignore_ascii_case(name))
}

fn is_classic_script_type(value: &str) -> bool {
    let kind = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    matches!(
        kind.as_str(),
        "" | "text/javascript"
            | "application/javascript"
            | "text/ecmascript"
            | "application/ecmascript"
    )
}

fn resource_output_path(value: &str, request_url: &str, output_root: &Path) -> Option<PathBuf> {
    let url = resolve_local_url(value, request_url)?;
    output_path_for_url_path(url.path(), output_root)
}

fn resolve_local_url(value: &str, request_url: &str) -> Option<url::Url> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }

    let request_path = request_url
        .split(['?', '#'])
        .next()
        .filter(|path| path.starts_with('/'))
        .unwrap_or("/");
    let base = url::Url::parse(&format!("http://tola.local{request_path}")).ok()?;
    let url = base.join(value).ok()?;

    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    if url.host_str() != Some("tola.local") {
        return None;
    }

    Some(url)
}

fn output_path_for_url_path(url_path: &str, output_root: &Path) -> Option<PathBuf> {
    let decoded = percent_decode_str(url_path).decode_utf8().ok()?;
    let rel = decoded.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }

    let mut clean = PathBuf::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    (!clean.as_os_str().is_empty()).then(|| output_root.join(clean))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn page_waits_for_missing_blocking_head_files_during_startup() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        std::fs::create_dir_all(output.join("post")).unwrap();
        let html = output.join("post/index.html");
        std::fs::write(
            &html,
            r#"<html><head>
<link rel="stylesheet" href="/styles/site.css?v=1">
<script src="/scripts/site/core.js"></script>
</head><body>Post</body></html>"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.build.output = output.clone();
        let ready = ServeReady::new();

        assert!(!page_ready(&html, "/post/", &config, &ready));

        std::fs::create_dir_all(output.join("styles")).unwrap();
        std::fs::write(output.join("styles/site.css"), "body{}").unwrap();
        assert!(!page_ready(&html, "/post/", &config, &ready));

        std::fs::create_dir_all(output.join("scripts/site")).unwrap();
        std::fs::write(output.join("scripts/site/core.js"), "").unwrap();
        assert!(page_ready(&html, "/post/", &config, &ready));
    }

    #[test]
    fn page_allows_missing_resources_after_startup_done() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        std::fs::create_dir_all(&output).unwrap();
        let html = output.join("index.html");
        std::fs::write(
            &html,
            r#"<html><head><link rel="stylesheet" href="/missing.css"></head><body>Home</body></html>"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.build.output = output;
        let ready = ServeReady::new();
        ready.set_startup_done();

        assert!(page_ready(&html, "/", &config, &ready));
    }

    #[test]
    fn head_resource_scan_ignores_nonblocking_and_external_resources() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let html = r#"<html><head>
<link rel="shortcut icon" href="/favicon.ico">
<link rel="stylesheet" href="https://example.com/site.css">
<link rel="stylesheet" href="/disabled.css" disabled>
<script src="/defer.js" defer></script>
<script src="/async.js" async></script>
<script src="/module.js" type="module"></script>
<script type="application/json" src="/data.json"></script>
<script src="/blocking.js"></script>
</head><body></body></html>"#;

        let resources = blocking_head_files(html, "/posts/hello/", &output);

        assert_eq!(resources, vec![output.join("blocking.js")]);
    }

    #[test]
    fn head_resource_scan_resolves_relative_and_prefixed_urls() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let html = r#"<html><head>
<link rel="stylesheet" href="local.css">
<script src="/docs/blog/scripts/app.js?v=1#x"></script>
</head><body></body></html>"#;

        let resources = blocking_head_files(html, "/docs/blog/posts/hello/", &output);

        assert_eq!(
            resources,
            vec![
                output.join("docs/blog/posts/hello/local.css"),
                output.join("docs/blog/scripts/app.js"),
            ]
        );
    }
}
