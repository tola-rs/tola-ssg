//! Browser URL semantics for links inside syndication content.

pub(super) fn absolute_url(destination: &str, page_url: &str) -> String {
    url::Url::parse(page_url)
        .and_then(|base| base.join(destination))
        .map(|url| url.to_string())
        .expect("validated feed destination resolves against the canonical document URL")
}
