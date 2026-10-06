//! Indexes realized URLs, logical document outputs, and fragment targets.

use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::output::semantics::ResponseMediaType;
use std::sync::Arc;
use tola_address::{OutputPath, RequiredResourceKind, UrlPath};

use super::{AddressResolution, HtmlPage, Resource, SiteAssetRoute};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AddressRegistrationError {
    #[error("two outputs claim the route `{url}`; give one page or asset a different route")]
    UrlAlreadyRegistered { url: UrlPath },
    #[error("two Bundle documents emit `{output}`; pass a different path to `document(...)`")]
    DocumentOutputAlreadyRegistered { output: OutputPath },
    #[error(
        "`{output}` contains the id `{fragment}` more than once; give each element a unique `id`"
    )]
    FragmentTargetRepeated {
        output: OutputPath,
        fragment: String,
    },
}

/// Complete address index for one realized site.
#[derive(Debug, Default)]
pub struct AddressSpace {
    by_url: FxHashMap<UrlPath, Resource>,
    by_output: FxHashMap<OutputPath, UrlPath>,
    /// Raw HTML output URL alias -> clean realized document URL.
    ///
    /// The alias never replaces the exact file URL, which stays canonical.
    output_aliases: FxHashMap<UrlPath, UrlPath>,
    fragments: FxHashMap<UrlPath, Arc<[String]>>,
}

/// Reads made by the authoritative resolver for one document's references.
///
/// Missing keys matter as much as present keys: a new direct route can shadow
/// an alias, and a previously missing target can appear without changing HTML.
#[derive(Debug, Default)]
pub(super) struct AddressDependencies {
    resources: FxHashMap<UrlPath, Option<bool>>,
    aliases: FxHashMap<UrlPath, Option<UrlPath>>,
    fragments: FxHashMap<UrlPath, FragmentDependencies>,
    media_types: FxHashMap<UrlPath, ResponseMediaType>,
}

#[derive(Debug, Default)]
struct FragmentDependencies {
    present: FxHashSet<String>,
    missing: Option<Arc<[String]>>,
}

impl AddressDependencies {
    pub(super) fn unchanged(
        &self,
        address: &AddressSpace,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        for (url, is_page) in &self.resources {
            cancellation.ensure_active()?;
            if address.by_url.get(url).map(Resource::is_page) != *is_page {
                return Ok(false);
            }
        }
        for (url, target) in &self.aliases {
            cancellation.ensure_active()?;
            if address.output_aliases.get(url) != target.as_ref() {
                return Ok(false);
            }
        }
        for (url, observed) in &self.fragments {
            cancellation.ensure_active()?;
            let Some(current) = address.fragments.get(url) else {
                return Ok(false);
            };
            if let Some(missing) = &observed.missing {
                if !Arc::ptr_eq(missing, current) && missing != current {
                    return Ok(false);
                }
            } else {
                for fragment in &observed.present {
                    cancellation.ensure_active()?;
                    if current.binary_search(fragment).is_err() {
                        return Ok(false);
                    }
                }
            }
        }
        for (url, observed) in &self.media_types {
            cancellation.ensure_active()?;
            let Some(Resource::Asset { route }) = address.by_url.get(url) else {
                return Ok(false);
            };
            if route.declaration().media_type() != observed {
                return Ok(false);
            }
        }
        cancellation.ensure_active()?;
        Ok(true)
    }

    pub(super) fn observe_media_type(&mut self, url: &UrlPath, media_type: &ResponseMediaType) {
        observe_once(&mut self.media_types, url, || media_type.clone());
    }

    fn observe_fragment(
        &mut self,
        url: &UrlPath,
        fragment: &str,
        available: &Arc<[String]>,
        found: bool,
    ) {
        let observed = self.fragments.entry(url.clone()).or_default();
        if found {
            if observed.missing.is_none() && !observed.present.contains(fragment) {
                observed.present.insert(fragment.to_owned());
            }
        } else if observed.missing.is_none() {
            observed.present.clear();
            observed.missing = Some(Arc::clone(available));
        }
    }
}

impl AddressSpace {
    /// Register one realized document and its final fragment identifiers.
    pub(super) fn register_page(
        &mut self,
        document: HtmlPage,
        fragments: impl IntoIterator<Item = String>,
    ) -> Result<(), AddressRegistrationError> {
        let permalink = document.permalink.clone();
        let output = document.output.clone();
        let output_alias = tola_address::asset_url_from_output(&output);
        if self.by_output.contains_key(&output) {
            return Err(AddressRegistrationError::DocumentOutputAlreadyRegistered { output });
        }
        if self.url_is_registered(&permalink) {
            return Err(AddressRegistrationError::UrlAlreadyRegistered { url: permalink });
        }
        if output_alias != permalink && self.url_is_registered(&output_alias) {
            return Err(AddressRegistrationError::UrlAlreadyRegistered { url: output_alias });
        }
        let mut fragment_set = FxHashSet::default();
        for fragment in fragments {
            if fragment_set.contains(&fragment) {
                return Err(AddressRegistrationError::FragmentTargetRepeated { output, fragment });
            }
            fragment_set.insert(fragment);
        }
        let resource = Resource::Page { document };
        self.register_resource(permalink.clone(), resource);
        self.by_output.insert(output, permalink.clone());
        if output_alias != permalink {
            self.output_aliases.insert(output_alias, permalink.clone());
        }
        let mut fragments = fragment_set.into_iter().collect::<Vec<_>>();
        fragments.sort_unstable();
        self.fragments.insert(permalink, fragments.into());
        Ok(())
    }

    pub(super) fn register_asset_route(
        &mut self,
        route: SiteAssetRoute,
    ) -> Result<(), AddressRegistrationError> {
        let url = route.url.clone();
        if self.url_is_registered(&url) {
            return Err(AddressRegistrationError::UrlAlreadyRegistered { url });
        }
        let resource = Resource::Asset { route };
        self.register_resource(url, resource);
        Ok(())
    }

    /// Serve directory-index routes for non-document outputs.
    ///
    /// Called after every exact registration so an explicitly published resource always keeps its
    /// route: direct lookup resolves before aliases, so a document or file that owns the route
    /// would still win, and recording the alias would only claim an address the build does not
    /// serve as an index.
    pub(super) fn register_asset_directory_indexes(
        &mut self,
        indexes: impl IntoIterator<Item = (UrlPath, UrlPath)>,
    ) {
        for (alias, exact) in indexes {
            if !self.url_is_registered(&alias) {
                self.output_aliases.insert(alias, exact);
            }
        }
    }

    fn register_resource(&mut self, url: UrlPath, resource: Resource) {
        let previous = self.by_url.insert(url, resource);
        debug_assert!(previous.is_none());
    }

    fn url_is_registered(&self, url: &UrlPath) -> bool {
        self.by_url.contains_key(url) || self.output_aliases.contains_key(url)
    }

    pub fn get_by_url(&self, url: &UrlPath) -> Option<&Resource> {
        self.by_url.get(url)
    }

    pub fn pages(&self) -> Vec<&HtmlPage> {
        let mut documents = self
            .by_url
            .values()
            .filter_map(|resource| match resource {
                Resource::Page { document } => Some(document),
                Resource::Asset { .. } => None,
            })
            .collect::<Vec<_>>();
        documents.sort_unstable_by(|left, right| left.permalink.cmp(&right.permalink));
        documents
    }

    pub fn resource_count(&self) -> usize {
        self.by_url.len()
    }

    pub fn resources(&self) -> Vec<(&UrlPath, &Resource)> {
        let mut resources = self.by_url.iter().collect::<Vec<_>>();
        resources.sort_unstable_by_key(|(url, _)| *url);
        resources
    }

    pub fn page_by_output(&self, output: &OutputPath) -> Option<&HtmlPage> {
        let url = self.by_output.get(output)?;
        match self.get_by_url(url)? {
            Resource::Page { document } => Some(document),
            Resource::Asset { .. } => None,
        }
    }

    /// Resolve one decoded site-root route and its serialized fragment.
    ///
    /// Resolve relative URLs, document bases, deployment mounts, and external schemes with
    /// [`tola_address::resolve_browser_reference`] before lookup: it decodes the escapes an
    /// address spells, so a literal `%` or `#` in an output filename reaches this method as the
    /// name character it is. `/x` addresses the file `x` and `/x/` addresses `x/index.html`, and
    /// the exact address is tried first, then the alias table an output shares with its other
    /// spelling.
    pub(super) fn resolve_route(
        &self,
        route: &UrlPath,
        fragment: &str,
        target: RequiredResourceKind,
        mut dependencies: Option<&mut AddressDependencies>,
    ) -> AddressResolution {
        let Some((url, resource)) = self.lookup_route(route, &mut dependencies) else {
            return AddressResolution::NotFound;
        };
        if target == RequiredResourceKind::NonDocumentResource && resource.is_page() {
            return AddressResolution::DocumentNotAllowed {
                reference: route.as_str().to_owned(),
                document: url.clone(),
            };
        }
        if fragment.is_empty() {
            AddressResolution::Found { url: url.clone() }
        } else {
            self.check_fragment_on_resource(resource, url, fragment, dependencies)
        }
    }

    /// The page the same address serves under its trailing-slash spelling.
    ///
    /// `/guide` addresses the file `guide`, while `/guide/` serves `guide/index.html`, so a site
    /// publishing the page and not the file answers the address its author meant. Only a reference
    /// that can name a document is offered one.
    ///
    /// Both the route read here and the route answered are decoded, so a literal `%` names a file
    /// rather than an escape; the caller renders the browser spelling of the answer.
    pub(super) fn directory_page_suggestion(
        &self,
        route: &UrlPath,
        target: RequiredResourceKind,
        dependencies: &mut Option<&mut AddressDependencies>,
    ) -> Option<UrlPath> {
        if target != RequiredResourceKind::DocumentOrResource || route.as_str().ends_with('/') {
            return None;
        }
        let directory = UrlPath::from_decoded(&format!("{}/", route.as_str())).ok()?;
        let (url, resource) = self.lookup_route(&directory, dependencies)?;
        resource.is_page().then(|| url.clone())
    }

    fn lookup_route<'a>(
        &'a self,
        route: &UrlPath,
        dependencies: &mut Option<&mut AddressDependencies>,
    ) -> Option<(&'a UrlPath, &'a Resource)> {
        self.lookup_resource(route, dependencies).or_else(|| {
            self.lookup_alias(route, dependencies)
                .and_then(|url| self.lookup_resource(url, dependencies))
        })
    }

    fn lookup_resource<'a>(
        &'a self,
        url: &UrlPath,
        dependencies: &mut Option<&mut AddressDependencies>,
    ) -> Option<(&'a UrlPath, &'a Resource)> {
        let found = self.by_url.get_key_value(url);
        if let Some(dependencies) = dependencies {
            observe_once(&mut dependencies.resources, url, || {
                found.map(|(_, resource)| resource.is_page())
            });
        }
        found
    }

    fn lookup_alias<'a>(
        &'a self,
        url: &UrlPath,
        dependencies: &mut Option<&mut AddressDependencies>,
    ) -> Option<&'a UrlPath> {
        let found = self.output_aliases.get(url);
        if let Some(dependencies) = dependencies {
            observe_once(&mut dependencies.aliases, url, || found.cloned());
        }
        found
    }

    fn check_fragment_on_resource(
        &self,
        resource: &Resource,
        url: &UrlPath,
        fragment: &str,
        dependencies: Option<&mut AddressDependencies>,
    ) -> AddressResolution {
        if !resource.is_page() {
            return AddressResolution::Found { url: url.clone() };
        }

        // Fragment directives select browser-managed content, not HTML ids.
        if fragment.contains(":~:") {
            return AddressResolution::Found { url: url.clone() };
        }

        let fragments = self
            .fragments
            .get(url)
            .expect("document registration installs its final fragment set");
        // HTML first tries the serialized spelling, then its percent-decoded value.
        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#the-indicated-part-of-the-document
        let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
        let found = [fragment, decoded.as_ref()].into_iter().find(|candidate| {
            fragments
                .binary_search_by(|id| id.as_str().cmp(candidate))
                .is_ok()
        });
        if let Some(found) = found {
            if let Some(dependencies) = dependencies {
                dependencies.observe_fragment(url, found, fragments, true);
            }
            return AddressResolution::Found { url: url.clone() };
        }
        if decoded.eq_ignore_ascii_case("top") {
            return AddressResolution::Found { url: url.clone() };
        }
        if let Some(dependencies) = dependencies {
            dependencies.observe_fragment(url, &decoded, fragments, false);
        }
        AddressResolution::FragmentNotFound {
            fragment: decoded.into_owned(),
            available: Arc::clone(fragments),
        }
    }
}

/// Record `value` for `key` only when `observed` has no entry for that key yet.
fn observe_once<K: std::hash::Hash + Eq + Clone, V: Clone>(
    observed: &mut FxHashMap<K, V>,
    key: &K,
    value: impl FnOnce() -> V,
) {
    if !observed.contains_key(key) {
        observed.insert(key.clone(), value());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn html_page(permalink: &str, output: &str) -> HtmlPage {
        HtmlPage {
            permalink: UrlPath::from_decoded(permalink).unwrap(),
            output: OutputPath::parse(output).unwrap(),
            properties: typst::model::DocumentInfo::default(),
            sources: Vec::new(),
        }
    }

    /// Resolve one reference the way final HTML reaches this index: an address decodes
    /// its escapes once, and the decoded route is looked up as written.
    fn resolve(
        space: &AddressSpace,
        reference: &str,
        target: RequiredResourceKind,
    ) -> AddressResolution {
        let reference = tola_address::SiteReference::parse(reference).unwrap();
        space.resolve_route(
            &reference.route,
            reference.fragment.as_deref().unwrap_or_default(),
            target,
            None,
        )
    }

    #[test]
    fn registered_routes_resolve_by_url() {
        let mut space = AddressSpace::default();
        let mut expected = Vec::new();
        for (permalink, output) in [
            ("/hello/", "hello/index.html"),
            ("/world/", "world/index.html"),
        ] {
            let page = html_page(permalink, output);
            expected.push((page.permalink.clone(), "page"));
            space.register_page(page, std::iter::empty()).unwrap();
        }
        let asset = site_asset_route("assets/logo.png", "/assets/logo.png");
        expected.push((asset.url.clone(), "asset"));
        space.register_asset_route(asset).unwrap();

        for (url, expected) in expected {
            let resource = space.get_by_url(&url).expect("registered route resolves");
            assert_eq!(
                match resource {
                    Resource::Page { .. } => "page",
                    Resource::Asset { .. } => "asset",
                },
                expected,
                "{url}"
            );
        }
    }

    fn site_asset_route(source: &str, url: &str) -> SiteAssetRoute {
        SiteAssetRoute {
            source: Some(PathBuf::from(source)),
            url: UrlPath::from_decoded(url).unwrap(),
            declaration: crate::output::semantics::OutputDeclaration::from_filesystem_source(
                std::path::Path::new(source),
            ),
        }
    }

    // Reference validation and the development server must agree on which addresses a build
    // serves, so the exact file URL stays canonical.
    #[test]
    fn directory_index_aliases_resolve() {
        let mut space = AddressSpace::default();
        let route = site_asset_route("external/index.html", "/external/index.html");
        let exact = route.url.clone();
        space.register_asset_route(route).unwrap();
        space.register_asset_directory_indexes([(
            UrlPath::parse("/external/").unwrap(),
            exact.clone(),
        )]);

        for reference in ["/external/", "/external/index.html"] {
            assert!(
                matches!(
                    resolve(&space, reference, RequiredResourceKind::DocumentOrResource),
                    AddressResolution::Found { url } if url == exact
                ),
                "{reference}"
            );
        }
        assert!(matches!(
            resolve(
                &space,
                "/nowhere/",
                RequiredResourceKind::DocumentOrResource
            ),
            AddressResolution::NotFound
        ));
    }

    #[test]
    fn explicit_route_beats_directory_alias() {
        let mut space = AddressSpace::default();
        let page = html_page("/deep/", "elsewhere/deep.html");
        let permalink = page.permalink.clone();
        space.register_page(page, std::iter::empty()).unwrap();
        let asset = site_asset_route("deep/index.html", "/deep/index.html");
        let exact = asset.url.clone();
        space.register_asset_route(asset).unwrap();
        space.register_asset_directory_indexes([(permalink.clone(), exact)]);

        assert!(matches!(
            resolve(&space, "/deep/", RequiredResourceKind::DocumentOrResource),
            AddressResolution::Found { url } if url == permalink
        ));
        assert!(matches!(
            resolve(&space, "/deep/index.html", RequiredResourceKind::DocumentOrResource),
            AddressResolution::Found { url } if url == UrlPath::parse("/deep/index.html").unwrap()
        ));
    }

    #[test]
    fn slashless_route_names_the_file() {
        let mut space = AddressSpace::default();
        let page = html_page("/guide/", "guide/index.html");
        let permalink = page.permalink.clone();
        space.register_page(page, std::iter::empty()).unwrap();
        space
            .register_asset_route(site_asset_route("guide", "/guide"))
            .unwrap();

        assert!(matches!(
            resolve(&space, "/guide", RequiredResourceKind::DocumentOrResource),
            AddressResolution::Found { url } if url == UrlPath::parse("/guide").unwrap()
        ));
        assert!(matches!(
            resolve(&space, "/guide/", RequiredResourceKind::DocumentOrResource),
            AddressResolution::Found { url } if url == permalink
        ));
    }

    #[test]
    fn escaped_names_resolve_from_their_address() {
        let mut space = AddressSpace::default();
        space
            .register_page(
                html_page("/100%-ready/", "100%-ready/index.html"),
                ["intro".to_owned()],
            )
            .unwrap();
        space
            .register_page(html_page("/a#b/", "a#b/index.html"), std::iter::empty())
            .unwrap();

        for (reference, permalink) in [
            ("/100%25-ready/", "/100%-ready/"),
            ("/100%25-ready/#intro", "/100%-ready/"),
            ("/100%25-ready/index.html", "/100%-ready/"),
            ("/a%23b/", "/a#b/"),
        ] {
            assert!(
                matches!(
                    resolve(&space, reference, RequiredResourceKind::DocumentOrResource),
                    AddressResolution::Found { url } if url.as_str() == permalink
                ),
                "{reference}"
            );
        }

        assert!(matches!(
            resolve(&space, "/100%25-ready/#missing", RequiredResourceKind::DocumentOrResource),
            AddressResolution::FragmentNotFound { fragment, .. } if fragment == "missing"
        ));
        assert!(matches!(
            resolve(
                &space,
                "/100%2525-ready/",
                RequiredResourceKind::DocumentOrResource
            ),
            AddressResolution::NotFound
        ));
    }

    #[test]
    fn duplicate_registration_changes_nothing() {
        let mut space = AddressSpace::default();
        let route = html_page("/hello/", "hello/index.html");
        let permalink = route.permalink.clone();
        assert!(matches!(
            space.register_page(route, ["same".to_owned(), "same".to_owned()]),
            Err(AddressRegistrationError::FragmentTargetRepeated { .. })
        ));
        assert!(space.get_by_url(&permalink).is_none());

        space
            .register_page(html_page("/same/", "first/index.html"), std::iter::empty())
            .unwrap();
        assert!(matches!(
            space.register_page(html_page("/same/", "second/index.html"), std::iter::empty()),
            Err(AddressRegistrationError::UrlAlreadyRegistered { .. })
        ));
        assert!(matches!(
            space.register_page(html_page("/third/", "first/index.html"), std::iter::empty()),
            Err(AddressRegistrationError::DocumentOutputAlreadyRegistered { .. })
        ));
        let resource = space
            .get_by_url(&UrlPath::parse("/same/").unwrap())
            .expect("the first page keeps its route");
        let Resource::Page { document } = resource else {
            panic!("/same/ must still serve the first page");
        };
        assert_eq!(document.output.as_str(), "first/index.html");
        assert!(
            space
                .get_by_url(&UrlPath::parse("/third/").unwrap())
                .is_none()
        );
    }

    #[test]
    fn every_document_alias_form_resolves() {
        for (permalink, output, fragments, reference) in [
            (
                "/hello/",
                "hello/index.html",
                vec!["hello world"],
                "/hello/#hello%20world",
            ),
            (
                "/post/",
                "post/index.html",
                vec!["intro"],
                "/post/index.html#intro",
            ),
            ("/post/", "a b/index.html", Vec::new(), "/a%20b/index.html"),
            (
                "/search/",
                "search/index.html",
                Vec::new(),
                "/search/?q=rust",
            ),
        ] {
            let mut space = AddressSpace::default();
            space
                .register_page(
                    html_page(permalink, output),
                    fragments.into_iter().map(str::to_owned),
                )
                .unwrap();

            assert!(
                matches!(
                    resolve(&space, reference, RequiredResourceKind::DocumentOrResource),
                    AddressResolution::Found { .. }
                ),
                "{reference}"
            );
        }
    }

    #[test]
    fn missing_fragments_share_sorted_ids() {
        let mut space = AddressSpace::default();
        let route = html_page("/hello/", "hello/index.html");
        space
            .register_page(route.clone(), ["z-last".to_owned(), "a-first".to_owned()])
            .unwrap();
        let mut previous = None;
        for fragment in ["missing", "another", "missing"] {
            let AddressResolution::FragmentNotFound { available, .. } = resolve(
                &space,
                &format!("/hello/#{fragment}"),
                RequiredResourceKind::DocumentOrResource,
            ) else {
                panic!("missing fragment resolved");
            };
            assert_eq!(available.as_ref(), ["a-first", "z-last"]);
            if let Some(previous) = &previous {
                assert!(Arc::ptr_eq(previous, &available));
            }
            previous = Some(available);
        }
    }

    #[test]
    fn html_fragments_follow_browser_targets() {
        let mut space = AddressSpace::default();
        space
            .register_page(
                html_page("/", "index.html"),
                ["part%20x", "hello world", "a#b"].map(str::to_owned),
            )
            .unwrap();

        for fragment in [
            "",
            "top",
            "TOP",
            "%74op",
            "part%20x",
            "part%2520x",
            "hello%20world",
            "a%23b",
            ":~:text=Hello",
            "section:~:text=Hello",
        ] {
            assert!(
                matches!(
                    resolve(
                        &space,
                        &format!("/#{fragment}"),
                        RequiredResourceKind::DocumentOrResource
                    ),
                    AddressResolution::Found { .. }
                ),
                "{fragment}"
            );
        }
        for fragment in ["missing", "hello%2520world", "%3A~%3Atext=Hello"] {
            assert!(
                matches!(
                    resolve(
                        &space,
                        &format!("/#{fragment}"),
                        RequiredResourceKind::DocumentOrResource
                    ),
                    AddressResolution::FragmentNotFound { .. }
                ),
                "{fragment}"
            );
        }
    }

    #[test]
    fn url_cannot_be_claimed_twice() {
        let mut space = AddressSpace::default();
        space
            .register_page(
                html_page("/first/", "shared/index.html"),
                std::iter::empty(),
            )
            .unwrap();
        let second = HtmlPage {
            permalink: UrlPath::parse("/shared/index.html").unwrap(),
            output: OutputPath::parse("second/index.html").unwrap(),
            properties: typst::model::DocumentInfo::default(),
            sources: Vec::new(),
        };
        assert!(matches!(
            space.register_page(second, std::iter::empty()),
            Err(AddressRegistrationError::UrlAlreadyRegistered { url })
                if url == UrlPath::parse("/shared/index.html").unwrap()
        ));

        let mut space = AddressSpace::default();
        space
            .register_page(html_page("/post/", "shared/index.html"), std::iter::empty())
            .unwrap();
        assert!(matches!(
            space
                .register_asset_route(site_asset_route("assets/shared.html", "/shared/index.html")),
            Err(AddressRegistrationError::UrlAlreadyRegistered { .. })
        ));
    }

    #[test]
    fn realized_urls_resolve_verbatim() {
        let mut space = AddressSpace::default();
        let page = html_page("/Mixed/", "Mixed/index.html");
        space
            .register_page(page.clone(), std::iter::empty())
            .unwrap();
        space
            .register_asset_route(SiteAssetRoute {
                source: None,
                url: UrlPath::parse("/paper.pdf").unwrap(),
                declaration: crate::output::semantics::OutputDeclaration::pdf_document(),
            })
            .unwrap();
        assert!(matches!(
            resolve(&space, "/Mixed/", RequiredResourceKind::DocumentOrResource),
            AddressResolution::Found { .. }
        ));
        assert!(matches!(
            resolve(&space, "/mixed/", RequiredResourceKind::DocumentOrResource),
            AddressResolution::NotFound
        ));
        assert!(matches!(
            resolve(
                &space,
                "/paper.pdf",
                RequiredResourceKind::DocumentOrResource
            ),
            AddressResolution::Found { .. }
        ));
        assert!(matches!(
            resolve(
                &space,
                "/paper.pdf#section",
                RequiredResourceKind::DocumentOrResource
            ),
            AddressResolution::Found { .. }
        ));

        assert!(matches!(
            resolve(
                &space,
                "/paper.pdf",
                RequiredResourceKind::NonDocumentResource
            ),
            AddressResolution::Found { .. }
        ));
        assert!(matches!(
            resolve(&space, "/Mixed/", RequiredResourceKind::NonDocumentResource),
            AddressResolution::DocumentNotAllowed { .. }
        ));
    }
}
