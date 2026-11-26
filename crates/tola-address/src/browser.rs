//! Browser URL classification shared by document graphs and reference checks.

use thiserror::Error;

use super::{SiteRef, SiteUrlMount, UrlPath};

/// Synthetic origin used to test whether a resolved reference stays local to the build.
const DOCUMENT_ORIGIN: &str = "https://tola-document.invalid";

/// Browser reference classified against its document and deployment mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserRef {
    /// Site-root reference inside the deployment mount, with optional query and fragment.
    Site(SiteRef),
    /// Same-origin reference outside the deployment mount.
    OutsideSite(String),
    /// Reference to another origin or a non-http scheme.
    External,
}

/// Invalid browser URL reference.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{reference}` is not a valid URL")]
pub struct BrowserRefError {
    reference: String,
}

/// Classify `raw` against `doc`, `mount`, and optional `<base href>`.
///
/// Any other origin (including configured `site.origin`) or non-http scheme is [`BrowserRef::External`].
pub fn classify_browser_ref(
    doc: &UrlPath,
    mount: &SiteUrlMount,
    base_href: Option<&str>,
    raw: &str,
) -> Result<BrowserRef, BrowserRefError> {
    let err = || BrowserRefError {
        reference: raw.to_owned(),
    };

    let fallback = doc_url(doc, mount).ok_or_else(err)?;
    let base = base_href
        .and_then(|val| doc_base(&fallback, val))
        .unwrap_or(fallback);
    let url = base.join(raw).map_err(|_| err())?;

    if url.origin().ascii_serialization() != DOCUMENT_ORIGIN {
        return Ok(BrowserRef::External);
    }

    let path = UrlPath::parse(url.path()).map_err(|_| err())?;
    let Some(route) = mount.strip(&path) else {
        return Ok(BrowserRef::OutsideSite(browser_value(&url)));
    };
    Ok(BrowserRef::Site(SiteRef {
        route,
        query: url.query().map(str::to_owned),
        fragment: url.fragment().map(str::to_owned),
    }))
}

/// Return whether `<base href>` keeps the build's own origin (rejected/invalid bases fall back to `true`).
pub fn is_local_base_href(doc: &UrlPath, mount: &SiteUrlMount, val: &str) -> bool {
    let Some(fallback) = doc_url(doc, mount) else {
        return true;
    };
    match doc_base(&fallback, val) {
        Some(base) => base.origin().ascii_serialization() == DOCUMENT_ORIGIN,
        None => true,
    }
}

fn doc_url(doc: &UrlPath, mount: &SiteUrlMount) -> Option<url::Url> {
    url::Url::parse(DOCUMENT_ORIGIN)
        .expect("document origin is a valid URL")
        .join(&mount.browser_path(doc))
        .ok()
}

fn doc_base(fallback: &url::Url, val: &str) -> Option<url::Url> {
    let base = fallback.join(val).ok()?;
    let ok = !base.cannot_be_a_base() && !matches!(base.scheme(), "data" | "javascript");
    ok.then_some(base)
}

fn browser_value(url: &url::Url) -> String {
    let mut out = url.path().to_owned();
    if let Some(q) = url.query() {
        out.push('?');
        out.push_str(q);
    }
    if let Some(frag) = url.fragment() {
        out.push('#');
        out.push_str(frag);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url_path(raw: &str) -> UrlPath {
        UrlPath::parse(raw).unwrap()
    }

    fn site_ref(encoded: &str) -> BrowserRef {
        BrowserRef::Site(SiteRef::parse(encoded).unwrap())
    }

    #[test]
    fn base_href_origin() {
        let page = url_path("/guide/");
        assert_eq!(
            classify_browser_ref(&page, &SiteUrlMount::root(), Some("/docs/"), "target/").unwrap(),
            site_ref("/docs/target/")
        );
        assert_eq!(
            classify_browser_ref(
                &page,
                &SiteUrlMount::root(),
                Some("https://example.test/docs/"),
                "target/"
            )
            .unwrap(),
            BrowserRef::External
        );
    }

    #[test]
    fn invalid_base_fallback() {
        let page = url_path("/guide/");
        for base in ["https://[", "data:text/plain,hello", "javascript:void(0)"] {
            assert_eq!(
                classify_browser_ref(&page, &SiteUrlMount::root(), Some(base), "target/").unwrap(),
                site_ref("/guide/target/")
            );
        }
    }

    #[test]
    fn external_urls() {
        let page = url_path("/guide/");
        for raw in [
            "https://example.com/target/",
            "//example.com/target/",
            "https://other.test/target/",
        ] {
            assert_eq!(
                classify_browser_ref(&page, &SiteUrlMount::root(), None, raw).unwrap(),
                BrowserRef::External,
                "{raw}"
            );
        }
    }

    #[test]
    fn mounted_site_ref() {
        let page = url_path("/guide/");
        let mount = SiteUrlMount::from_base_path("/docs/").unwrap();
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "target/").unwrap(),
            site_ref("/guide/target/")
        );
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "/docs/target/").unwrap(),
            site_ref("/target/")
        );
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "/%64ocs/target/").unwrap(),
            site_ref("/target/")
        );
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "/target/").unwrap(),
            BrowserRef::OutsideSite("/target/".into())
        );
    }

    #[test]
    fn query_and_fragment() {
        let page = url_path("/posts/hello/");
        let mount = SiteUrlMount::root();
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "../shared%20image.png?q=1#preview").unwrap(),
            BrowserRef::Site(SiteRef {
                route: UrlPath::parse("/posts/shared%20image.png").unwrap(),
                query: Some("q=1".into()),
                fragment: Some("preview".into()),
            })
        );
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "?q=rust").unwrap(),
            BrowserRef::Site(SiteRef {
                route: UrlPath::parse("/posts/hello/").unwrap(),
                query: Some("q=rust".into()),
                fragment: None,
            })
        );
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "#intro").unwrap(),
            BrowserRef::Site(SiteRef {
                route: UrlPath::parse("/posts/hello/").unwrap(),
                query: None,
                fragment: Some("intro".into()),
            })
        );
    }

    #[test]
    fn escaped_path_decode() {
        let page = url_path("/index.html");
        let mount = SiteUrlMount::root();
        for (raw, route) in [("/100%25-ready/", "/100%-ready/"), ("/a%23b/", "/a#b/")] {
            assert_eq!(
                classify_browser_ref(&page, &mount, None, raw).unwrap(),
                BrowserRef::Site(SiteRef {
                    route: UrlPath::from_decoded(route).unwrap(),
                    query: None,
                    fragment: None,
                }),
                "{raw}"
            );
        }
        assert_eq!(
            classify_browser_ref(&page, &mount, None, "/#a%23b").unwrap(),
            BrowserRef::Site(SiteRef {
                route: UrlPath::parse("/").unwrap(),
                query: None,
                fragment: Some("a%23b".into()),
            })
        );
    }
}
