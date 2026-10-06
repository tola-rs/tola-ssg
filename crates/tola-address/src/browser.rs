//! Browser URL resolution shared by document graphs and reference validation.

use thiserror::Error;

use super::{SiteReference, SiteUrlMount, UrlPath};

/// The origin a document's own URL has while a reference resolves.
///
/// The document's decoded route and the deployment mount stand in for a real address, so a
/// reference that keeps this origin is one the build itself can resolve; a reference that names
/// another origin belongs to whatever serves it, which this build cannot observe.
const DOCUMENT_ORIGIN: &str = "https://tola-document.invalid";

/// A browser reference resolved against its document and deployment mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedBrowserReference {
    /// A site-root reference inside the deployment mount, with its query and fragment.
    Site(SiteReference),
    /// A document-relative reference outside the deployment mount.
    OutsideSite(String),
    /// A reference naming another origin, or a non-http scheme.
    External,
}

/// A reference that cannot be resolved as a browser URL.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{reference}` is not a valid URL")]
pub struct BrowserReferenceError {
    reference: String,
}

/// Apply browser URL resolution before looking up an address in a sealed site.
///
/// `document` is a decoded site-root route and `base_href` is the final HTML document base,
/// when present. A reference that resolves to the document's own synthesized origin is local: it
/// is a [`Site`](ResolvedBrowserReference::Site) reference when it stays inside the deployment
/// mount and [`OutsideSite`](ResolvedBrowserReference::OutsideSite) when it does not. A reference
/// naming any other origin — including the configured `site.origin` — and every non-http scheme
/// is [`External`](ResolvedBrowserReference::External): it belongs to whatever serves that
/// address, which the build cannot observe, so it is never validated.
///
/// A document base naming another origin therefore makes the whole page external, which is why
/// producers that report on unvalidated pages ask [`base_href_keeps_build_origin`] first.
pub fn resolve_browser_reference(
    document: &UrlPath,
    mount: &SiteUrlMount,
    base_href: Option<&str>,
    reference: &str,
) -> Result<ResolvedBrowserReference, BrowserReferenceError> {
    let invalid = || BrowserReferenceError {
        reference: reference.to_owned(),
    };

    let document_origin =
        url::Url::parse(DOCUMENT_ORIGIN).expect("the document origin is a valid URL");
    let fallback = document_origin
        .join(&mount.browser_path(document))
        .map_err(|_| invalid())?;
    let base = base_href
        .and_then(|value| fallback.join(value).ok())
        .filter(is_allowed_document_base)
        .unwrap_or(fallback);
    let resolved = base.join(reference).map_err(|_| invalid())?;

    if resolved.origin().ascii_serialization() != DOCUMENT_ORIGIN {
        return Ok(ResolvedBrowserReference::External);
    }

    let path = UrlPath::parse(resolved.path()).map_err(|_| invalid())?;
    let Some(site_route) = mount.strip(&path) else {
        return Ok(ResolvedBrowserReference::OutsideSite(browser_value(
            &resolved,
        )));
    };
    Ok(ResolvedBrowserReference::Site(SiteReference {
        route: site_route,
        query: resolved.query().map(str::to_owned),
        fragment: resolved.fragment().map(str::to_owned),
    }))
}

/// Whether a document's `<base href>` keeps this build's own origin.
///
/// The value joins exactly as [`resolve_browser_reference`] joins it. A base that cannot be
/// joined, or that a browser document base rejects, is ignored there, so it leaves this build's
/// origin in place and answers `true`. Only a base naming another origin answers `false`, and
/// then every reference of that page resolves [`External`](ResolvedBrowserReference::External).
pub fn base_href_keeps_build_origin(document: &UrlPath, mount: &SiteUrlMount, value: &str) -> bool {
    let document_origin =
        url::Url::parse(DOCUMENT_ORIGIN).expect("the document origin is a valid URL");
    let Ok(fallback) = document_origin.join(&mount.browser_path(document)) else {
        return true;
    };
    match fallback.join(value).ok().filter(is_allowed_document_base) {
        Some(base) => base.origin().ascii_serialization() == DOCUMENT_ORIGIN,
        None => true,
    }
}

/// The host-relative value a browser would request for an inherited-origin reference.
fn browser_value(resolved: &url::Url) -> String {
    let mut value = resolved.path().to_owned();
    if let Some(query) = resolved.query() {
        value.push('?');
        value.push_str(query);
    }
    if let Some(fragment) = resolved.fragment() {
        value.push('#');
        value.push_str(fragment);
    }
    value
}

fn is_allowed_document_base(url: &url::Url) -> bool {
    !url.cannot_be_a_base() && !matches!(url.scheme(), "data" | "javascript")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::url_path;

    #[test]
    fn base_href_decides_internal_or_external() {
        let page = url_path("/guide/");
        assert_eq!(
            resolve_browser_reference(&page, &SiteUrlMount::root(), Some("/docs/"), "target/")
                .unwrap(),
            resolved_site("/docs/target/")
        );
        assert_eq!(
            resolve_browser_reference(
                &page,
                &SiteUrlMount::root(),
                Some("https://example.test/docs/"),
                "target/"
            )
            .unwrap(),
            ResolvedBrowserReference::External
        );
    }

    #[test]
    fn invalid_base_uses_document_url() {
        let page = url_path("/guide/");
        for base in ["https://[", "data:text/plain,hello", "javascript:void(0)"] {
            assert_eq!(
                resolve_browser_reference(&page, &SiteUrlMount::root(), Some(base), "target/")
                    .unwrap(),
                resolved_site("/guide/target/")
            );
        }
    }

    #[test]
    fn absolute_urls_stay_external() {
        let page = url_path("/guide/");
        for reference in [
            "https://example.com/target/",
            "//example.com/target/",
            "https://other.test/target/",
        ] {
            assert_eq!(
                resolve_browser_reference(&page, &SiteUrlMount::root(), None, reference).unwrap(),
                ResolvedBrowserReference::External,
                "{reference}"
            );
        }
    }

    #[test]
    fn mounted_site_maps_browser_to_site() {
        let page = url_path("/guide/");
        let mount = SiteUrlMount::from_base_path("/docs/").unwrap();
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "target/").unwrap(),
            resolved_site("/guide/target/")
        );
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "/docs/target/").unwrap(),
            resolved_site("/target/")
        );
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "/%64ocs/target/").unwrap(),
            resolved_site("/target/")
        );
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "/target/").unwrap(),
            ResolvedBrowserReference::OutsideSite("/target/".into())
        );
    }

    #[test]
    fn resolution_keeps_query_and_fragment() {
        let page = url_path("/posts/hello/");
        let mount = SiteUrlMount::root();
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "../shared%20image.png?q=1#preview")
                .unwrap(),
            ResolvedBrowserReference::Site(SiteReference {
                route: UrlPath::parse("/posts/shared%20image.png").unwrap(),
                query: Some("q=1".into()),
                fragment: Some("preview".into()),
            })
        );
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "?q=rust").unwrap(),
            ResolvedBrowserReference::Site(SiteReference {
                route: UrlPath::parse("/posts/hello/").unwrap(),
                query: Some("q=rust".into()),
                fragment: None,
            })
        );
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "#intro").unwrap(),
            ResolvedBrowserReference::Site(SiteReference {
                route: UrlPath::parse("/posts/hello/").unwrap(),
                query: None,
                fragment: Some("intro".into()),
            })
        );
    }

    #[test]
    fn escaped_paths_decode_once() {
        let page = url_path("/index.html");
        let mount = SiteUrlMount::root();
        for (reference, route) in [("/100%25-ready/", "/100%-ready/"), ("/a%23b/", "/a#b/")] {
            assert_eq!(
                resolve_browser_reference(&page, &mount, None, reference).unwrap(),
                ResolvedBrowserReference::Site(SiteReference {
                    route: UrlPath::from_decoded(route).unwrap(),
                    query: None,
                    fragment: None,
                }),
                "{reference}"
            );
        }
        assert_eq!(
            resolve_browser_reference(&page, &mount, None, "/#a%23b").unwrap(),
            ResolvedBrowserReference::Site(SiteReference {
                route: UrlPath::parse("/").unwrap(),
                query: None,
                fragment: Some("a%23b".into()),
            })
        );
    }

    #[test]
    fn non_http_schemes_stay_external() {
        let page = url_path("/guide/");
        for reference in [
            "mailto:user@example.com",
            "data:text/plain,hello",
            "tel:+123",
        ] {
            assert_eq!(
                resolve_browser_reference(&page, &SiteUrlMount::root(), None, reference).unwrap(),
                ResolvedBrowserReference::External,
                "{reference}"
            );
        }
    }

    /// The site reference a rooted encoded path names, for assertions.
    fn resolved_site(encoded: &str) -> ResolvedBrowserReference {
        ResolvedBrowserReference::Site(SiteReference::parse(encoded).unwrap())
    }
}
