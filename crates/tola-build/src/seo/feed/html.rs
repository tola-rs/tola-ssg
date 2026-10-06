//! Browser URL semantics for links inside syndication content.

pub(super) fn absolute_url(destination: &str, page_url: &str) -> String {
    url::Url::parse(page_url)
        .and_then(|base| base.join(destination))
        .map(|url| url.to_string())
        .expect("validated feed destination resolves against the canonical document URL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_destinations_resolve_against_page() {
        let page = "https://example.com/blog/posts/%E4%B8%AD%E6%96%87/";
        for (destination, expected) in [
            (
                "#top",
                "https://example.com/blog/posts/%E4%B8%AD%E6%96%87/#top",
            ),
            (
                "next/",
                "https://example.com/blog/posts/%E4%B8%AD%E6%96%87/next/",
            ),
            ("/blog/about/", "https://example.com/blog/about/"),
            ("/about/", "https://example.com/about/"),
            ("https://typst.app/", "https://typst.app/"),
        ] {
            assert_eq!(absolute_url(destination, page), expected);
        }
    }
}
