//! References resolved from the sealed site output.

pub(crate) mod diagnostic;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::output::semantics::ResponseMediaType;
use crate::site::space::AddressDependencies;
use crate::site::{AddressResolution, HtmlPage, Resource, SiteIndex};
use tola_address::{
    LinkKind, OutputPath, RequiredResourceKind, SiteReference, SiteUrlMount, UrlPath,
};

/// All references found in the final HTML outputs of one sealed site.
#[derive(Clone, Default)]
pub struct References(Arc<ReferenceSet>);

#[derive(Default)]
struct ReferenceSet {
    url_mount: SiteUrlMount,
    groups: Vec<Arc<DocumentReferences>>,
    incoming: BTreeMap<UrlPath, Vec<usize>>,
    outgoing: BTreeMap<OutputPath, usize>,
    len: usize,
}

/// Source meaning and the exact target reads that justify one immutable group.
struct DocumentReferences {
    output: OutputPath,
    permalink: UrlPath,
    inventory: Arc<tola_typst::HtmlDocumentInventory>,
    dependencies: Arc<AddressDependencies>,
    references: Vec<Reference>,
    incoming: BTreeMap<UrlPath, Vec<usize>>,
    /// The document base that names another origin, with its source span, when the page has one.
    ///
    /// Every relative reference of such a page resolves external, so the page's reference checks
    /// are skipped; retaining the base lets the projection report that instead of staying silent.
    unchecked_base: Option<UncheckedBase>,
}

/// A document base naming another origin, and where the page wrote it.
#[derive(Clone)]
pub(crate) struct UncheckedBase {
    value: String,
    origin: Option<crate::diagnostic::Location>,
}

struct ReferenceIter<'a> {
    groups: std::slice::Iter<'a, Arc<DocumentReferences>>,
    current: std::slice::Iter<'a, Reference>,
    remaining: usize,
}

impl<'a> Iterator for ReferenceIter<'a> {
    type Item = &'a Reference;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(reference) = self.current.next() {
                self.remaining -= 1;
                return Some(reference);
            }
            self.current = self.groups.next()?.references.iter();
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for ReferenceIter<'_> {}
impl std::iter::FusedIterator for ReferenceIter<'_> {}

/// The destination class a reference belongs to, used by diagnostics and inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceCategory {
    Navigation,
    Resource,
    Fragment,
    External,
}

impl ReferenceCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::Resource => "resource",
            Self::Fragment => "fragment",
            Self::External => "external",
        }
    }
}

/// Resolution result for one URL found in final HTML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceResolution {
    Found { target: ReferenceTarget },
    External,
    Unresolved { reason: UnresolvedReferenceReason },
}

/// The canonical site resource and browser suffix selected by a valid reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceTarget {
    url: UrlPath,
    query: Option<String>,
    fragment: Option<String>,
}

impl ReferenceTarget {
    fn new(url: UrlPath, reference: SiteReference) -> Self {
        let fragment = reference.decoded_fragment().map(Cow::into_owned);
        Self {
            url,
            query: reference.query,
            fragment,
        }
    }

    /// Without a deployment mount, query, or fragment.
    pub fn url(&self) -> &UrlPath {
        &self.url
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Decoded fragment without the leading hash sign; its meaning belongs to the target media type.
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
}

/// Precise failure that prevented a local reference from resolving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnresolvedReferenceReason {
    OutsideSiteMount {
        destination: String,
        required_prefix: String,
    },
    InvalidBrowserReference {
        message: String,
    },
    TargetMissing {
        /// Browser address the same route serves under its trailing-slash spelling, when the
        /// author likely meant that address.
        suggestion: Option<String>,
    },
    FragmentMissing {
        fragment: String,
        available: Arc<[String]>,
    },
    DocumentNotAllowed {
        reference: String,
        document: tola_address::UrlPath,
    },
    MediaTypeMismatch {
        reference_use: tola_typst::HtmlReferenceUse,
        required: ResponseMediaType,
        declared: ResponseMediaType,
    },
}

impl UnresolvedReferenceReason {
    /// Clause that continues the destination named by the rendered diagnostic.
    pub fn message(&self, category: ReferenceCategory) -> String {
        match self {
            Self::OutsideSiteMount {
                required_prefix, ..
            } => {
                format!(
                    "is outside `site.base-path`, so the site is served under `{required_prefix}`"
                )
            }
            Self::InvalidBrowserReference { message } => message.clone(),
            Self::TargetMissing { .. } => match category {
                ReferenceCategory::Resource => "is not a file this site publishes".into(),
                _ => "is not a page this site publishes".into(),
            },
            Self::FragmentMissing { fragment, .. } => {
                format!("has no element with the id `{fragment}`")
            }
            Self::DocumentNotAllowed { .. } => {
                "is an HTML page, but this attribute needs a file".to_string()
            }
            Self::MediaTypeMismatch {
                reference_use,
                required,
                declared,
            } => format!(
                "is used as {}, which needs `{}`, but the file is served as `{}`",
                reference_expectation(*reference_use).usage,
                required.as_str(),
                declared.as_str()
            ),
        }
    }

    pub fn help(&self, category: ReferenceCategory) -> Option<String> {
        match self {
            Self::OutsideSiteMount {
                required_prefix, ..
            } => Some(format!(
                "Start root-relative URLs with `{required_prefix}` (from `site.base-path`) or make the link relative"
            )),
            Self::TargetMissing {
                suggestion: Some(suggestion),
            } => Some(format!("Point the link at `{suggestion}`")),
            Self::TargetMissing { suggestion: None } => Some(match category {
                ReferenceCategory::Resource => {
                    "Declare it in `assets.files` or `assets.trees` in `tola.toml`, or take its URL from `asset-url()`".into()
                }
                _ => {
                    "Point the link at a page that exists, or create the page that publishes it"
                        .into()
                }
            }),
            Self::FragmentMissing { available, .. } if !available.is_empty() => Some(format!(
                "Choose an existing id: {}",
                crate::diagnostic::bounded_listing(available, ("id", "ids"))
            )),
            Self::DocumentNotAllowed { .. } => {
                Some("Point it at a generated file such as a stylesheet, script, or image".into())
            }
            Self::MediaTypeMismatch { .. } => Some(
                "Give the file the extension this attribute needs, or use the matching attribute"
                    .into(),
            ),
            Self::InvalidBrowserReference { .. } | Self::FragmentMissing { .. } => None,
        }
    }

    pub(crate) const fn is_media_type_mismatch(&self) -> bool {
        matches!(self, Self::MediaTypeMismatch { .. })
    }
}

/// One outgoing reference together with its final site-wide resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    page: OutputPath,
    destination: String,
    html_context: String,
    category: ReferenceCategory,
    resolution: ReferenceResolution,
    origin: Option<crate::diagnostic::Location>,
}

impl Reference {
    pub fn page(&self) -> &OutputPath {
        &self.page
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn html_context(&self) -> &str {
        &self.html_context
    }

    /// Absent when the element came from outside the site, such as a package.
    pub fn origin(&self) -> Option<&crate::diagnostic::Location> {
        self.origin.as_ref()
    }

    pub fn category(&self) -> ReferenceCategory {
        self.category
    }

    pub fn resolution(&self) -> &ReferenceResolution {
        &self.resolution
    }
}

impl References {
    pub(crate) fn from_site(
        snapshot: &SiteIndex,
        url_mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
        cancellation: &BuildCancellation,
        previous: Option<&References>,
    ) -> Result<Self, BuildCancelled> {
        cancellation.ensure_active()?;
        let previous = previous.filter(|previous| &previous.0.url_mount == url_mount);
        // An unchanged group keeps its predecessors by Arc rather than copying references.
        let mut changed_groups = None;
        let mut page_count = 0;
        for (page, inventory) in snapshot.html_pages() {
            cancellation.ensure_active()?;
            let cached = previous.and_then(|previous| {
                previous
                    .0
                    .outgoing
                    .get(&page.output)
                    .map(|&position| &previous.0.groups[position])
            });
            let reusable = if let Some(cached) = cached {
                cached.same_reference_semantics(page, inventory, cancellation)?
                    && cached
                        .dependencies
                        .unchanged(snapshot.address(), cancellation)?
            } else {
                false
            };
            let refreshed = if reusable {
                cached
                    .expect("reusable groups have prior evidence")
                    .refresh_origins(inventory, world, cancellation)?
            } else {
                None
            };
            let unchanged_prefix = reusable
                && refreshed.is_none()
                && previous
                    .and_then(|previous| previous.0.groups.get(page_count))
                    .zip(cached)
                    .is_some_and(|(at_position, cached)| Arc::ptr_eq(at_position, cached));
            if changed_groups.is_some() || !unchanged_prefix {
                let groups = changed_groups.get_or_insert_with(|| {
                    previous.map_or_else(Vec::new, |previous| {
                        previous.0.groups[..page_count].to_vec()
                    })
                });
                groups.push(if reusable {
                    refreshed.map_or_else(
                        || Arc::clone(cached.expect("reusable groups have prior evidence")),
                        Arc::new,
                    )
                } else {
                    Arc::new(DocumentReferences::resolve(
                        snapshot,
                        page,
                        inventory,
                        url_mount,
                        world,
                        cancellation,
                    )?)
                });
            }
            page_count += 1;
        }
        cancellation.ensure_active()?;
        let groups = match changed_groups {
            Some(groups) => groups,
            None => match previous {
                Some(previous) if previous.0.groups.len() == page_count => {
                    return Ok(previous.clone());
                }
                Some(previous) => previous.0.groups[..page_count].to_vec(),
                None => Vec::new(),
            },
        };
        let mut incoming = BTreeMap::<UrlPath, Vec<usize>>::new();
        let mut outgoing = BTreeMap::new();
        let mut len = 0;
        for (position, group) in groups.iter().enumerate() {
            cancellation.ensure_active()?;
            outgoing.insert(group.output.clone(), position);
            len += group.references.len();
            for url in group.incoming.keys() {
                cancellation.ensure_active()?;
                incoming.entry(url.clone()).or_default().push(position);
            }
        }
        cancellation.ensure_active()?;
        Ok(Self(Arc::new(ReferenceSet {
            url_mount: url_mount.clone(),
            groups,
            incoming,
            outgoing,
            len,
        })))
    }

    pub fn len(&self) -> usize {
        self.0.len
    }

    pub fn is_empty(&self) -> bool {
        self.0.len == 0
    }

    pub fn references(&self) -> impl ExactSizeIterator<Item = &Reference> {
        ReferenceIter {
            groups: self.0.groups.iter(),
            current: [].iter(),
            remaining: self.0.len,
        }
    }

    /// Every page whose document base keeps its references from being checked.
    pub(crate) fn unchecked_bases(&self) -> impl Iterator<Item = (&OutputPath, &UncheckedBase)> {
        self.0.groups.iter().filter_map(|group| {
            group
                .unchecked_base
                .as_ref()
                .map(|base| (&group.output, base))
        })
    }

    /// In page and DOM order.
    pub fn incoming(&self, url: &UrlPath) -> impl Iterator<Item = &Reference> {
        self.0
            .incoming
            .get_key_value(url)
            .into_iter()
            .flat_map(move |(url, groups)| {
                groups.iter().flat_map(move |&position| {
                    let group = &self.0.groups[position];
                    group.incoming[url]
                        .iter()
                        .map(move |&position| &group.references[position])
                })
            })
    }

    /// Includes unresolved references.
    pub fn outgoing(&self, page: &OutputPath) -> impl Iterator<Item = &Reference> {
        self.0
            .outgoing
            .get(page)
            .into_iter()
            .flat_map(|&position| self.0.groups[position].references.iter())
    }
}

impl DocumentReferences {
    fn same_reference_semantics(
        &self,
        page: &HtmlPage,
        inventory: &Arc<tola_typst::HtmlDocumentInventory>,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        if self.output != page.output || self.permalink != page.permalink {
            return Ok(false);
        }
        if Arc::ptr_eq(&self.inventory, inventory) {
            return Ok(true);
        }
        if self
            .inventory
            .base_href()
            .map(tola_typst::HtmlBaseHref::value)
            != inventory.base_href().map(tola_typst::HtmlBaseHref::value)
            || self.inventory.references().len() != inventory.references().len()
        {
            return Ok(false);
        }
        // Source positions are projected separately; only URL semantics determine reuse here.
        for (old, new) in self
            .inventory
            .references()
            .iter()
            .zip(inventory.references())
        {
            cancellation.ensure_active()?;
            if old.tag() != new.tag()
                || old.attribute() != new.attribute()
                || old.destination() != new.destination()
                || old.relation() != new.relation()
                || old.reference_use() != new.reference_use()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn refresh_origins(
        &self,
        inventory: &Arc<tola_typst::HtmlDocumentInventory>,
        world: &tola_typst::TypstWorld,
        cancellation: &BuildCancellation,
    ) -> Result<Option<Self>, BuildCancelled> {
        let mut references = None;
        for (position, reference) in inventory.references().iter().enumerate() {
            cancellation.ensure_active()?;
            let origin = crate::compiler::source_location(world, reference.span());
            if self.references[position].origin != origin {
                references.get_or_insert_with(|| self.references.clone())[position].origin = origin;
            }
        }
        // The base's own span moves with its source line exactly as a reference's does.
        let unchecked_base = self.unchecked_base.as_ref().map(|base| UncheckedBase {
            value: base.value.clone(),
            origin: inventory
                .base_href()
                .and_then(|base| crate::compiler::source_location(world, base.span())),
        });
        let refreshed = references.is_some()
            || unchecked_base.as_ref().and_then(|base| base.origin.clone())
                != self
                    .unchecked_base
                    .as_ref()
                    .and_then(|base| base.origin.clone());
        Ok(refreshed.then(|| Self {
            output: self.output.clone(),
            permalink: self.permalink.clone(),
            inventory: Arc::clone(inventory),
            dependencies: Arc::clone(&self.dependencies),
            references: references.unwrap_or_else(|| self.references.clone()),
            incoming: self.incoming.clone(),
            unchecked_base,
        }))
    }

    fn resolve(
        snapshot: &SiteIndex,
        page: &HtmlPage,
        inventory: &Arc<tola_typst::HtmlDocumentInventory>,
        url_mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
        cancellation: &BuildCancellation,
    ) -> Result<Self, BuildCancelled> {
        let mut references = Vec::with_capacity(inventory.references().len());
        let mut dependencies = AddressDependencies::default();
        let mut incoming = BTreeMap::<UrlPath, Vec<usize>>::new();
        for reference in inventory.references() {
            cancellation.ensure_active()?;
            let resolved = resolve_reference(
                snapshot,
                page,
                inventory,
                reference,
                url_mount,
                world,
                &mut dependencies,
            );
            if let ReferenceResolution::Found { target } = &resolved.resolution {
                incoming
                    .entry(target.url.clone())
                    .or_default()
                    .push(references.len());
            }
            references.push(resolved);
        }
        cancellation.ensure_active()?;
        Ok(Self {
            output: page.output.clone(),
            permalink: page.permalink.clone(),
            inventory: Arc::clone(inventory),
            dependencies: Arc::new(dependencies),
            references,
            incoming,
            unchecked_base: unchecked_base(inventory, page, url_mount, world),
        })
    }
}

/// The document base that keeps this build from checking the page, when it has one.
fn unchecked_base(
    inventory: &tola_typst::HtmlDocumentInventory,
    page: &HtmlPage,
    url_mount: &SiteUrlMount,
    world: &tola_typst::TypstWorld,
) -> Option<UncheckedBase> {
    let base = inventory.base_href()?;
    (!tola_address::base_href_keeps_build_origin(&page.permalink, url_mount, base.value())).then(
        || UncheckedBase {
            value: base.value().to_owned(),
            origin: crate::compiler::source_location(world, base.span()),
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_reference(
    snapshot: &SiteIndex,
    page: &HtmlPage,
    inventory: &tola_typst::HtmlDocumentInventory,
    reference: &tola_typst::HtmlReference,
    url_mount: &SiteUrlMount,
    world: &tola_typst::TypstWorld,
    dependencies: &mut AddressDependencies,
) -> Reference {
    let effective = tola_address::resolve_browser_reference(
        &page.permalink,
        url_mount,
        inventory.base_href().map(tola_typst::HtmlBaseHref::value),
        reference.destination(),
    );
    let default_category = reference_expectation(reference.reference_use()).category;
    let (category, resolution) = match effective {
        Ok(tola_address::ResolvedBrowserReference::External) => {
            (ReferenceCategory::External, ReferenceResolution::External)
        }
        Ok(tola_address::ResolvedBrowserReference::OutsideSite(destination)) => (
            default_category,
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::OutsideSiteMount {
                    destination,
                    required_prefix: format!("/{}/", url_mount.as_str()),
                },
            },
        ),
        Err(_) => (
            default_category,
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::InvalidBrowserReference {
                    message: "is not a valid URL".to_string(),
                },
            },
        ),
        Ok(tola_address::ResolvedBrowserReference::Site(site)) => {
            match snapshot.address().resolve_route(
                &site.route,
                site.fragment.as_deref().unwrap_or_default(),
                reference_target(reference),
                Some(&mut *dependencies),
            ) {
                AddressResolution::Found { url } => {
                    let resolution = resolved_media_type(
                        snapshot,
                        reference,
                        ReferenceTarget::new(url, site),
                        dependencies,
                    );
                    let category = if matches!(resolution, ReferenceResolution::Found { .. })
                        && matches!(
                            LinkKind::parse(reference.destination()),
                            LinkKind::Fragment(_)
                        ) {
                        ReferenceCategory::Fragment
                    } else {
                        default_category
                    };
                    (category, resolution)
                }
                AddressResolution::NotFound => (
                    default_category,
                    ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::TargetMissing {
                            suggestion: snapshot
                                .address()
                                .directory_page_suggestion(
                                    &site.route,
                                    reference_target(reference),
                                    &mut Some(&mut *dependencies),
                                )
                                .map(|route| {
                                    SiteReference { route, ..site }.to_browser_url(url_mount, None)
                                }),
                        },
                    },
                ),
                AddressResolution::FragmentNotFound {
                    fragment,
                    available,
                    ..
                } => (
                    ReferenceCategory::Fragment,
                    ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::FragmentMissing {
                            fragment,
                            available,
                        },
                    },
                ),
                AddressResolution::DocumentNotAllowed {
                    reference,
                    document,
                } => (
                    default_category,
                    ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::DocumentNotAllowed {
                            reference,
                            document,
                        },
                    },
                ),
            }
        }
    };

    Reference {
        page: page.output.clone(),
        destination: reference.destination().to_owned(),
        html_context: reference_context(reference),
        category,
        resolution,
        origin: crate::compiler::source_location(world, reference.span()),
    }
}

/// Category, destination constraint, media requirement, and wording of one reference use.
struct ReferenceExpectation {
    category: ReferenceCategory,
    target: RequiredResourceKind,
    media_type: Option<ResponseMediaType>,
    usage: &'static str,
}

/// What each way of using a reference requires.
const fn reference_expectation(
    reference_use: tola_typst::HtmlReferenceUse,
) -> ReferenceExpectation {
    use tola_typst::HtmlReferenceUse as HtmlUse;
    match reference_use {
        HtmlUse::Navigation => ReferenceExpectation {
            category: ReferenceCategory::Navigation,
            target: RequiredResourceKind::DocumentOrResource,
            media_type: None,
            usage: "a navigation link",
        },
        HtmlUse::GenericResource => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: None,
            usage: "an embedded resource",
        },
        HtmlUse::Prefetch => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::DocumentOrResource,
            media_type: None,
            usage: "a prefetch link",
        },
        HtmlUse::Stylesheet => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::CSS),
            usage: "a stylesheet",
        },
        HtmlUse::ClassicScript => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::JAVASCRIPT),
            usage: "a classic script",
        },
        HtmlUse::ModuleScript => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::JAVASCRIPT),
            usage: "a module script",
        },
        HtmlUse::ModulePreload => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::JAVASCRIPT),
            usage: "a module preload link",
        },
    }
}

fn reference_target(reference: &tola_typst::HtmlReference) -> RequiredResourceKind {
    let expectation = reference_expectation(reference.reference_use());
    let embeds_a_document = matches!(
        reference.reference_use(),
        tola_typst::HtmlReferenceUse::GenericResource
    ) && matches!(
        (reference.tag(), reference.attribute()),
        ("object", "data") | ("embed", "src") | ("a", "ping") | ("area", "ping")
    );
    if embeds_a_document {
        RequiredResourceKind::DocumentOrResource
    } else {
        expectation.target
    }
}

fn resolved_media_type(
    snapshot: &SiteIndex,
    reference: &tola_typst::HtmlReference,
    target: ReferenceTarget,
    dependencies: &mut AddressDependencies,
) -> ReferenceResolution {
    let Some(required) = reference_expectation(reference.reference_use()).media_type else {
        return ReferenceResolution::Found { target };
    };
    let Resource::Asset { route } = snapshot
        .address()
        .get_by_url(target.url())
        .expect("a found URL has a registered resource")
    else {
        unreachable!("typed resource references reject document targets before media validation");
    };
    let declared = route.declaration().media_type();
    dependencies.observe_media_type(target.url(), declared);
    if resource_media_matches(declared, &required) {
        ReferenceResolution::Found { target }
    } else {
        ReferenceResolution::Unresolved {
            reason: UnresolvedReferenceReason::MediaTypeMismatch {
                reference_use: reference.reference_use(),
                required,
                declared: declared.clone(),
            },
        }
    }
}

fn resource_media_matches(declared: &ResponseMediaType, required: &ResponseMediaType) -> bool {
    if required.essence() != ResponseMediaType::JAVASCRIPT.essence() {
        return declared.essence() == required.essence();
    }
    tola_typst::html::is_javascript_mime_type(declared.essence())
}

fn reference_context(reference: &tola_typst::HtmlReference) -> String {
    match reference.relation() {
        Some(relation) => format!(
            "{}[{} rel={relation}]",
            reference.tag(),
            reference.attribute()
        ),
        None => format!("{}[{}]", reference.tag(), reference.attribute()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    /// Resolve every reference of `site` without a previous revision.
    fn site_references(
        site: &SiteIndex,
        mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
    ) -> References {
        References::from_site(site, mount, world, &BuildCancellation::new(), None).unwrap()
    }

    #[test]
    fn resource_media_matches_by_essence() {
        let stylesheet = ResponseMediaType::parse("text/css; charset=iso-8859-1").unwrap();
        assert!(resource_media_matches(&stylesheet, &ResponseMediaType::CSS));
        let script = ResponseMediaType::parse("application/javascript; charset=utf-8").unwrap();
        assert!(resource_media_matches(
            &script,
            &ResponseMediaType::JAVASCRIPT
        ));
        assert!(!resource_media_matches(
            &stylesheet,
            &ResponseMediaType::JAVASCRIPT
        ));
        let track = ResponseMediaType::parse("text/vtt").unwrap();
        assert!(!resource_media_matches(&track, &ResponseMediaType::CSS));
    }

    fn candidate(
        source: &str,
    ) -> anyhow::Result<(
        TempDir,
        SiteIndex,
        crate::output::graph::OutputGraph,
        tola_typst::TypstWorld,
    )> {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("site.typ");
        std::fs::write(&main, source).unwrap();
        let world = tola_typst::TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::default())
            .unwrap();
        let cancellation = tola_typst::BundleCancellation::default();
        let compilation = tola_typst::compile_bundle_world(&world, &cancellation).unwrap();
        let document_outputs = compilation
            .documents()
            .map(|document| {
                crate::compiler::documents::output_path_for_virtual(document.path()).unwrap()
            })
            .collect();
        let mut documents = Vec::new();
        let mut inventories = std::collections::BTreeMap::new();
        for document in compilation
            .documents()
            .filter(|document| document.kind() == tola_typst::BundleDocumentKind::Html)
        {
            let output =
                crate::compiler::documents::output_path_for_virtual(document.path()).unwrap();
            let permalink = tola_address::route_for_output(&output);
            let inventory = document.html_inventory(&cancellation).unwrap().unwrap();
            documents.push(crate::site::HtmlPage {
                permalink,
                output: output.clone(),
                properties: typst::model::DocumentInfo::default(),
                sources: document.source_ids(),
            });
            inventories.insert(output, inventory);
        }
        let export = compilation.export_default(&cancellation, None).unwrap();
        let mut graph = crate::output::graph::OutputGraphBuilder::new();
        crate::compiler::outputs::insert_bundle(
            &mut graph,
            export.entries().iter().cloned(),
            "test",
            &document_outputs,
        )
        .unwrap();
        let graph = graph.finish();
        let snapshot = SiteIndex::from_output_graph(&graph, documents, inventories)?;
        Ok((directory, snapshot, graph, world))
    }

    fn reindex(
        source: &SiteIndex,
        graph: &crate::output::graph::OutputGraph,
        mut update_route: impl FnMut(&mut HtmlPage),
    ) -> SiteIndex {
        let html_outputs = graph
            .outputs()
            .iter()
            .filter(|output| output.kind() == crate::output::graph::OutputKind::HtmlDocument)
            .map(|output| output.path())
            .collect::<std::collections::BTreeSet<_>>();
        let mut documents = Vec::new();
        let mut inventories = BTreeMap::new();
        for (page, inventory) in source.html_pages() {
            if html_outputs.contains(&page.output) {
                let mut page = page.clone();
                update_route(&mut page);
                inventories.insert(page.output.clone(), Arc::clone(inventory));
                documents.push(page);
            }
        }
        SiteIndex::from_output_graph(graph, documents, inventories).unwrap()
    }

    fn assert_matches_fresh(
        site: &SiteIndex,
        mount: &SiteUrlMount,
        previous: &References,
        config: &crate::config::ResolvedSiteConfig,
        world: &tola_typst::TypstWorld,
    ) -> References {
        let cancellation = BuildCancellation::new();
        let fresh = References::from_site(site, mount, world, &cancellation, None).unwrap();
        let cached =
            References::from_site(site, mount, world, &cancellation, Some(previous)).unwrap();
        assert_eq!(
            cached.references().collect::<Vec<_>>(),
            fresh.references().collect::<Vec<_>>(),
        );
        let mut pages = previous
            .references()
            .map(|reference| reference.page().clone())
            .collect::<BTreeSet<_>>();
        pages.extend(site.html_pages().map(|(page, _)| page.output.clone()));
        for page in pages {
            assert_eq!(
                cached.outgoing(&page).collect::<Vec<_>>(),
                fresh.outgoing(&page).collect::<Vec<_>>(),
            );
        }
        let mut urls = BTreeSet::new();
        for references in [previous, &cached, &fresh] {
            urls.extend(references.references().filter_map(
                |reference| match reference.resolution() {
                    ReferenceResolution::Found { target } => Some(target.url().clone()),
                    _ => None,
                },
            ));
        }
        for url in urls {
            assert_eq!(
                cached.incoming(&url).collect::<Vec<_>>(),
                fresh.incoming(&url).collect::<Vec<_>>(),
            );
        }
        assert_eq!(
            diagnostic::diagnostics(&cached, config, &cancellation).unwrap(),
            diagnostic::diagnostics(&fresh, config, &cancellation).unwrap(),
        );
        cached
    }

    fn assert_group_shared(previous: &References, cached: &References, output: &str) {
        let output = OutputPath::parse(output).unwrap();
        assert!(std::ptr::eq(
            previous.outgoing(&output).next().unwrap(),
            cached.outgoing(&output).next().unwrap(),
        ));
    }

    #[test]
    fn cached_diagnostics_follow_source_positions() {
        let source = "#document(\"index.html\")[\n  #html.a(href: \"/missing/\")[Missing]\n]";
        let (directory, site, _graph, world) = candidate(source).unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let mount = SiteUrlMount::root();
        let previous = site_references(&site, &mount, &world);
        assert_eq!(
            previous.references().next().unwrap().origin().unwrap().line,
            Some(2)
        );

        let (_directory, changed, _graph, world) = candidate(&format!("\n{source}")).unwrap();
        let cached = assert_matches_fresh(&changed, &mount, &previous, &config, &world);
        assert_eq!(
            cached.references().next().unwrap().origin().unwrap().line,
            Some(3)
        );
        let unchanged = assert_matches_fresh(&changed, &mount, &cached, &config, &world);
        assert_group_shared(&cached, &unchanged, "index.html");
    }

    #[test]
    fn cached_groups_track_fragment_repairs() {
        let source = |fragments: &str| {
            format!(
                r#"#document("kept.html")[#html.a(href: "/target/#kept")[Kept]]
#document("removed.html")[#html.a(href: "/target/#gone")[Gone]]
#document("missing.html")[
  #html.a(href: "/target/#missing")[First]
  #html.a(href: "/target/#missing")[Second]
]
#document("stable.html")[#html.a(href: "https://example.test/")[External]]
#document("target/index.html")[{fragments}]"#,
            )
        };
        let (directory, site, _graph, world) = candidate(&source(
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "gone")[Gone]"#,
        ))
        .unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let mount = SiteUrlMount::root();
        let mut previous = site_references(&site, &mount, &world);
        for fragments in [
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "gone")[Gone] #html.h2(id: "extra")[Extra]"#,
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "extra")[Extra]"#,
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "missing")[Repaired]"#,
        ] {
            let (_directory, changed, _graph, world) = candidate(&source(fragments)).unwrap();
            let cached = assert_matches_fresh(&changed, &mount, &previous, &config, &world);
            assert_group_shared(&previous, &cached, "kept.html");
            assert_group_shared(&previous, &cached, "stable.html");
            previous = cached;
        }
        let missing = OutputPath::parse("missing.html").unwrap();
        assert!(
            previous.outgoing(&missing).all(|reference| matches!(
                reference.resolution(),
                ReferenceResolution::Found { .. }
            ))
        );
        let removed = OutputPath::parse("removed.html").unwrap();
        assert!(matches!(
            previous.outgoing(&removed).next().unwrap().resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::FragmentMissing { .. }
            }
        ));
    }

    #[test]
    fn absolute_url_on_the_configured_origin_stays_external() {
        let (_directory, site, _graph, world) = candidate(
            r#"#document("index.html")[#html.a(href: "https://example.test/guide/")[Absolute]]
#document("guide/index.html")[Guide]"#,
        )
        .unwrap();
        let mount = SiteUrlMount::root();
        let page = OutputPath::parse("index.html").unwrap();
        let cancellation = BuildCancellation::new();
        let references = References::from_site(&site, &mount, &world, &cancellation, None).unwrap();
        // The build cannot observe what another site serves on its own origin, so an absolute
        // URL is never resolved against this build's outputs even when it names `site.origin`.
        assert_eq!(
            references.outgoing(&page).next().unwrap().resolution(),
            &ReferenceResolution::External
        );
        // The build proves nothing about what another site serves, so no diagnostic is produced.
        let config = crate::config::tests::load_test_config(
            _directory.path(),
            "[site]\norigin = \"https://example.test\"\n",
        );
        let diagnostics = crate::site::references::diagnostic::diagnostics(
            &references,
            &config,
            &BuildCancellation::new(),
        )
        .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn external_base_href_reports_the_unchecked_page() {
        let source = r#"#document("index.html", html.html(
  html.head(html.base(href: "https://other.test/"))
  + html.body(html.a(href: "/missing/")[Missing])
))"#;
        let (directory, site, _graph, world) = candidate(source).unwrap();
        let mount = SiteUrlMount::root();
        let page = OutputPath::parse("index.html").unwrap();
        let references =
            References::from_site(&site, &mount, &world, &BuildCancellation::new(), None).unwrap();
        // Every relative reference of the page resolves external, so none of them is checked.
        assert_eq!(
            references.outgoing(&page).next().unwrap().resolution(),
            &ReferenceResolution::External
        );
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let diagnostics = crate::site::references::diagnostic::diagnostics(
            &references,
            &config,
            &BuildCancellation::new(),
        )
        .unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(
            diagnostics[0].code,
            crate::codes::reference::BASE_HREF_EXTERNAL_ORIGIN
        );
        assert!(diagnostics[0].message.contains("https://other.test/"));
    }

    #[test]
    fn unresolved_reference_reports_its_source() {
        let (directory, site, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/nowhere.html")[Missing]
]"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let index = site_references(&site, &SiteUrlMount::root(), &world);
        let diagnostics = crate::site::references::diagnostic::diagnostics(
            &index,
            &config,
            &BuildCancellation::new(),
        )
        .unwrap();

        let diagnostic = diagnostics.first().expect("the missing page is reported");
        let location = diagnostic
            .location
            .as_ref()
            .expect("the reference is located in its source");
        assert_eq!(location.path, "site.typ");
        assert_eq!((location.line, location.column), (Some(2), Some(4)));
        let range = location
            .range
            .expect("the element keeps its authored range");
        assert_eq!((range.start.line, range.start.character), (1, 3));
        assert_eq!((range.end.line, range.end.character), (1, 41));
        assert!(diagnostic.notes.is_empty(), "{:?}", diagnostic.notes);
    }

    #[test]
    fn cached_groups_track_alias_precedence() {
        use crate::output::graph::{OutputFile, OutputGraphBuilder};
        use crate::output::semantics::OutputDeclaration;

        let (directory, source, original, world) = candidate(
            r#"#document("links/index.html")[
  #html.a(href: "/target/index.html?view=card")[Alias]
  #html.a(href: "/target/")[Clean]
  #html.img(src: "/target/index.html")
]
#document("stable.html")[#html.a(href: "/stable-target/")[Stable]]
#document("stable-target/index.html")[Stable]
#document("target/index.html")[Target]"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let mount = SiteUrlMount::root();
        let mut previous = References::default();
        for (declaration, route, expected) in [
            (None, None, None),
            (
                Some(OutputDeclaration::html_document()),
                None,
                Some("/target/"),
            ),
            (
                Some(OutputDeclaration::html_document()),
                Some("/moved/"),
                Some("/moved/"),
            ),
            (
                Some(OutputDeclaration::opaque(ResponseMediaType::HTML)),
                None,
                Some("/target/index.html"),
            ),
            (None, None, None),
        ] {
            let mut graph = OutputGraphBuilder::new();
            for output in original.outputs() {
                match output.path().as_str() {
                    "target/index.html" => {
                        if let Some(declaration) = &declaration {
                            graph
                                .insert(OutputFile::from_byte_owner(
                                    output.path().clone(),
                                    declaration.clone(),
                                    output.owner().clone(),
                                    output.bytes_owner(),
                                ))
                                .unwrap();
                        }
                    }
                    _ => graph.insert(output.clone()).unwrap(),
                }
            }
            let graph = graph.finish();
            let site = reindex(&source, &graph, |page| {
                if page.output.as_str() == "target/index.html"
                    && let Some(route) = route
                {
                    page.permalink = UrlPath::parse(route).unwrap();
                }
            });
            let cached = assert_matches_fresh(&site, &mount, &previous, &config, &world);
            if !previous.is_empty() {
                assert_group_shared(&previous, &cached, "stable.html");
            }
            let links = OutputPath::parse("links/index.html").unwrap();
            let alias = cached.outgoing(&links).next().unwrap();
            match expected {
                Some(expected) => {
                    let ReferenceResolution::Found { target } = alias.resolution() else {
                        panic!("alias did not resolve: {alias:?}");
                    };
                    assert_eq!(target.url().as_str(), expected);
                    assert_eq!(target.query(), Some("view=card"));
                }
                None => assert_eq!(
                    alias.resolution(),
                    &ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::TargetMissing { suggestion: None },
                    },
                ),
            }
            previous = cached;
        }
        let mut graph = OutputGraphBuilder::new();
        for output in original.outputs().iter().filter(|output| {
            matches!(
                output.path().as_str(),
                "stable.html" | "stable-target/index.html"
            )
        }) {
            graph.insert(output.clone()).unwrap();
        }
        let graph = graph.finish();
        let site = reindex(&source, &graph, |_| {});
        let without_links = assert_matches_fresh(&site, &mount, &previous, &config, &world);
        assert_group_shared(&previous, &without_links, "stable.html");
        let empty = assert_matches_fresh(
            &SiteIndex::default(),
            &mount,
            &without_links,
            &config,
            &world,
        );
        assert!(empty.is_empty());
    }

    #[test]
    fn only_typed_uses_see_media_changes() {
        use crate::output::graph::OutputGraphBuilder;
        use crate::output::semantics::OutputDeclaration;

        let (directory, source, original, world) = candidate(
            r#"#document("typed.html", html.html(html.head(
  html.link(rel: "stylesheet", href: "/typed.bin")
)))
#document("plain.html")[#html.a(href: "/typed.bin")[Download]]"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let mount = SiteUrlMount::root();
        let bytes: Arc<[u8]> = Arc::from(b"same response bytes".as_slice());
        let mut previous = References::default();
        for media_type in [
            ResponseMediaType::CSS,
            ResponseMediaType::JAVASCRIPT,
            ResponseMediaType::parse("text/javascript; charset=utf-8").unwrap(),
            ResponseMediaType::CSS,
        ] {
            let mut graph = OutputGraphBuilder::new();
            graph.insert_graph(&original).unwrap();
            graph
                .insert_configured_asset(
                    "mapped-resource",
                    OutputPath::parse("typed.bin").unwrap(),
                    OutputDeclaration::opaque(media_type.clone()),
                    Arc::clone(&bytes),
                )
                .unwrap();
            let graph = graph.finish();
            let output = graph
                .outputs()
                .iter()
                .find(|output| output.path().as_str() == "typed.bin")
                .unwrap();
            assert_eq!(output.bytes(), bytes.as_ref());
            let site = reindex(&source, &graph, |_| {});
            let cached = assert_matches_fresh(&site, &mount, &previous, &config, &world);
            if !previous.is_empty() {
                assert_group_shared(&previous, &cached, "plain.html");
            }
            let typed = OutputPath::parse("typed.html").unwrap();
            let resolution = cached.outgoing(&typed).next().unwrap().resolution();
            if media_type == ResponseMediaType::CSS {
                assert!(matches!(resolution, ReferenceResolution::Found { .. }));
            } else {
                assert!(matches!(
                    resolution,
                    ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::MediaTypeMismatch { declared, .. }
                    } if declared == &media_type
                ));
            }
            previous = cached;
        }
    }

    #[test]
    fn cached_groups_resolve_under_every_base() {
        let directory = TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let mut previous = References::default();
        for (base, destination) in [
            ("", "../target/"),
            ("", "../missing/"),
            ("/docs/", "target/"),
            ("https://tola-first.invalid/docs/", "target/"),
        ] {
            let (_directory, source, graph, world) = candidate(&format!(
                r#"#document("source/index.html", html.html(
  html.head(html.base(href: "{base}"))
  + html.body(
    html.a(href: "{destination}")[Target]
    + html.a(href: "http://[")[Invalid]
  )
))
#document("target/index.html")[Target]"#,
            ))
            .unwrap();
            for mount in [
                SiteUrlMount::root(),
                SiteUrlMount::from_base_path("/docs/").unwrap(),
            ] {
                for permalink in ["/source/", "/source/deeper/"] {
                    let site = reindex(&source, &graph, |page| {
                        if page.output.as_str() == "source/index.html" {
                            page.permalink = UrlPath::parse(permalink).unwrap();
                        }
                    });
                    previous = assert_matches_fresh(&site, &mount, &previous, &config, &world);
                }
            }
        }
        let source = OutputPath::parse("source/index.html").unwrap();
        assert_eq!(
            previous.outgoing(&source).next().unwrap().resolution(),
            &ReferenceResolution::External,
        );
    }

    #[test]
    fn cached_failures_take_the_current_policy() {
        use crate::config::ReferenceLevel;
        use crate::diagnostic::Severity;

        let (directory, site, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/missing/")[Missing]
  #html.img(src: "/missing.png")
  #html.a(href: "/target/#missing")[Fragment]
]
#document("target/index.html")[#html.h1(id: "present")[Present]]"#,
        )
        .unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        let mount = SiteUrlMount::root();
        let previous = site_references(&site, &mount, &world);
        let errors =
            diagnostic::diagnostics(&previous, &config, &BuildCancellation::new()).unwrap();
        assert_eq!(errors.len(), 3);
        assert!(
            errors
                .iter()
                .all(|diagnostic| diagnostic.severity == Severity::Error)
        );
        config.build.references.navigation = ReferenceLevel::Warn;
        config.build.references.resources = ReferenceLevel::Warn;
        config.build.references.fragments = ReferenceLevel::Warn;
        let cached = assert_matches_fresh(&site, &mount, &previous, &config, &world);
        assert_group_shared(&previous, &cached, "index.html");
        let warnings =
            diagnostic::diagnostics(&cached, &config, &BuildCancellation::new()).unwrap();
        assert_eq!(warnings.len(), 3);
        assert!(
            warnings
                .iter()
                .all(|diagnostic| diagnostic.severity == Severity::Warning)
        );
    }

    #[test]
    fn cancellation_preempts_reference_checks() {
        let (directory, site, _graph, world) =
            candidate(r#"#document("index.html")[#html.a(href: "/missing/")[Missing]]"#).unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let mount = SiteUrlMount::root();
        let cached = site_references(&site, &mount, &world);
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        for previous in [None, Some(&cached)] {
            assert!(matches!(
                References::from_site(&site, &mount, &world, &cancellation, previous),
                Err(BuildCancelled)
            ));
        }
        assert!(matches!(
            diagnostic::diagnostics(&cached, &config, &cancellation),
            Err(BuildCancelled)
        ));
    }

    #[test]
    fn final_dom_reference_resolves() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html")[#html.a(href: "/post/#intro")[Post]]
#document("post/index.html")[#html.h1(id: "intro")[Intro]]"#,
        )
        .unwrap();

        let index = site_references(&snapshot, &SiteUrlMount::root(), &world);

        assert_eq!(index.references().len(), 1);
        assert!(matches!(
            index.references().next().unwrap().resolution(),
            ReferenceResolution::Found { .. }
        ));
        assert_eq!(index.references().next().unwrap().html_context(), "a[href]");
    }

    #[test]
    fn backlinks_share_one_canonical_target() {
        let (_directory, site, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/docs/post/index.html?view=card#part%2520x")[Alias]
  #html.a(href: "/docs/post/?view=list#part%2520x")[Clean]
  #html.a(href: "/docs/missing/")[Missing]
]
#document("post/index.html")[#html.h1(id: "part%20x")[Post]]"#,
        )
        .unwrap();
        let references = site_references(
            &site,
            &SiteUrlMount::from_base_path("/docs/").unwrap(),
            &world,
        );
        let url = UrlPath::parse("/post/").unwrap();
        let incoming = references.incoming(&url).collect::<Vec<_>>();
        assert_eq!(incoming.len(), 2);
        for (reference, query) in incoming.into_iter().zip(["view=card", "view=list"]) {
            let ReferenceResolution::Found { target } = reference.resolution() else {
                panic!("expected a resolved backlink");
            };
            assert_eq!(target.url(), &url);
            assert_eq!(target.query(), Some(query));
            assert_eq!(target.fragment(), Some("part%20x"));
            assert_eq!(reference.page().as_str(), "index.html");
        }
        assert_eq!(
            references
                .outgoing(&OutputPath::parse("index.html").unwrap())
                .count(),
            3
        );
        assert!(
            references
                .incoming(&UrlPath::parse("/missing/").unwrap())
                .next()
                .is_none()
        );
    }

    #[test]
    fn escaped_output_filenames_resolve() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/100%25.html")[Percent-named page]
  #html.a(href: "/c%23notes.html")[Hash-named page]
]
#document("100%.html")[Percent-named page]
#document("c#notes.html")[Hash-named page]"#,
        )
        .unwrap();
        let references = site_references(&snapshot, &SiteUrlMount::root(), &world);

        let page = OutputPath::parse("index.html").unwrap();
        let resolved = references
            .outgoing(&page)
            .map(|reference| match reference.resolution() {
                ReferenceResolution::Found { target } => target.url().as_str(),
                resolution => panic!("escaped output filename did not resolve: {resolution:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(resolved, ["/100%.html", "/c#notes.html"]);
    }

    #[test]
    fn escaped_filename_fragment_names_available_ids() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html")[#html.a(href: "/c%23notes.html#missing")[Missing fragment]]
#document("c#notes.html")[#html.h1(id: "intro")[Intro]]"#,
        )
        .unwrap();
        let references = site_references(&snapshot, &SiteUrlMount::root(), &world);

        let page = OutputPath::parse("index.html").unwrap();
        let reference = references.outgoing(&page).next().unwrap();
        let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
            panic!("missing fragment resolved: {reference:?}");
        };
        assert!(matches!(
            reason,
            UnresolvedReferenceReason::FragmentMissing { fragment, available }
                if fragment == "missing" && available.as_ref() == ["intro"]
        ));
    }

    #[test]
    fn fragment_failures_follow_target_requirements() {
        use crate::codes::reference::{FRAGMENT_MISSING, NAVIGATION_MISSING, RESOURCE_MISSING};
        use crate::config::ReferenceLevel;
        use crate::diagnostic::Severity;

        for (source, category, code, severity) in [
            (
                r##"#document("index.html")[#html.script(src: "#intro") #html.h1(id: "intro")[Intro]]"##,
                ReferenceCategory::Resource,
                RESOURCE_MISSING,
                Severity::Error,
            ),
            (
                r##"#document("index.html")[#html.script(src: "#missing")]"##,
                ReferenceCategory::Resource,
                RESOURCE_MISSING,
                Severity::Error,
            ),
            (
                r##"#document("index.html", html.html(
  html.head(html.base(href: "/missing/"))
  + html.body(html.a(href: "#intro")[Missing page])
))"##,
                ReferenceCategory::Navigation,
                NAVIGATION_MISSING,
                Severity::Error,
            ),
            (
                r#"#document("index.html")[#html.a(href: "/missing/#fragment")[Missing page]]"#,
                ReferenceCategory::Navigation,
                NAVIGATION_MISSING,
                Severity::Error,
            ),
            (
                r#"#document("index.html")[#html.a(href: "/post/#missing")[Missing fragment]]
#document("post/index.html")[#html.h1(id: "available")[Post]]"#,
                ReferenceCategory::Fragment,
                FRAGMENT_MISSING,
                Severity::Warning,
            ),
        ] {
            let (directory, snapshot, _graph, world) = candidate(source).unwrap();
            let mut config = crate::config::tests::load_test_config(directory.path(), "");
            config.build.references.fragments = ReferenceLevel::Warn;
            let index = site_references(&snapshot, &SiteUrlMount::root(), &world);
            assert_eq!(index.references().next().unwrap().category(), category);
            let diagnostics =
                diagnostic::diagnostics(&index, &config, &BuildCancellation::new()).unwrap();
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, code);
            assert_eq!(diagnostics[0].severity, severity);
        }
    }

    #[test]
    fn slashless_links_preserve_target_suffixes() {
        for (mount, href, expected) in [
            (SiteUrlMount::root(), "/guide", "/guide/"),
            (
                SiteUrlMount::from_base_path("/docs/").unwrap(),
                "/docs/guide",
                "/docs/guide/",
            ),
            (
                SiteUrlMount::from_base_path("/docs/").unwrap(),
                "/docs/guide?view=card#intro%2520part",
                "/docs/guide/?view=card#intro%2520part",
            ),
        ] {
            let (_directory, snapshot, _graph, world) = candidate(&format!(
                r#"#document("index.html")[#html.a(href: "{href}")[Guide]]
#document("guide/index.html")[#html.h1(id: "intro%20part")[Guide]]"#
            ))
            .unwrap();
            let index = site_references(&snapshot, &mount, &world);
            let reference = index.references().next().unwrap();
            let ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::TargetMissing { suggestion },
            } = reference.resolution()
            else {
                panic!("{href} resolved: {reference:?}");
            };
            assert_eq!(suggestion.as_deref(), Some(expected), "{href}");
        }
    }

    #[test]
    fn percent_named_page_suggestion_resolves() {
        for (output, href, suggestion, route) in [
            (
                "100%-ready/index.html",
                "/100%25-ready",
                "/100%25-ready/",
                "/100%-ready/",
            ),
            ("a%41/index.html", "/a%2541", "/a%2541/", "/a%41/"),
        ] {
            let (_directory, snapshot, _graph, world) = candidate(&format!(
                r#"#document("index.html")[#html.a(href: "{href}")[Page]]
#document("{output}")[Target]"#
            ))
            .unwrap();
            let index = site_references(&snapshot, &SiteUrlMount::root(), &world);
            let reference = index.references().next().unwrap();
            let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
                panic!("{href} resolved: {reference:?}");
            };
            assert_eq!(
                reason.help(ReferenceCategory::Navigation),
                Some(format!("Point the link at `{suggestion}`")),
                "{href}"
            );

            let suggested = tola_address::SiteReference::parse(suggestion).unwrap();
            assert!(
                matches!(
                    snapshot.address().resolve_route(
                        &suggested.route,
                        "",
                        RequiredResourceKind::DocumentOrResource,
                        None,
                    ),
                    AddressResolution::Found { url } if url.as_str() == route
                ),
                "{suggestion} resolves to {route}"
            );
        }
    }

    #[test]
    fn missing_resources_name_the_declarations() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html")[#html.img(src: "/assets/missing.png", alt: "missing")]"#,
        )
        .unwrap();
        let index = site_references(&snapshot, &SiteUrlMount::root(), &world);
        let reference = index.references().next().unwrap();
        assert_eq!(reference.category(), ReferenceCategory::Resource);
        let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
            panic!("missing resource resolved");
        };
        assert_eq!(
            reason.message(ReferenceCategory::Resource),
            "is not a file this site publishes"
        );
        assert!(
            reason
                .help(ReferenceCategory::Resource)
                .is_some_and(|help| help.contains("`assets.files`")),
            "{:?}",
            reason.help(ReferenceCategory::Resource)
        );
    }

    #[test]
    fn each_missing_fragment_is_reported() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/post/#missing")[First]
  #html.a(href: "/post/#missing")[Second]
]
#document("post/index.html")[#html.h1(id: "z")[Last] #html.h2(id: "a")[First]]"#,
        )
        .unwrap();
        let index = site_references(&snapshot, &SiteUrlMount::root(), &world);
        assert_eq!(index.references().len(), 2);
        let mut previous = None;
        for reference in index.references() {
            let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
                panic!("missing fragment resolved");
            };
            let UnresolvedReferenceReason::FragmentMissing {
                fragment,
                available,
            } = reason
            else {
                panic!("missing fragment lost its diagnostic");
            };
            assert_eq!(fragment, "missing");
            assert_eq!(available.as_ref(), ["a", "z"]);
            if let Some(previous) = previous {
                assert!(Arc::ptr_eq(previous, available));
            }
            previous = Some(available);
        }
    }

    #[test]
    fn media_fragments_keep_their_target() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"
#document("index.html")[
  #link(<pdf-part>)[PDF section]
  #html.a(href: "/clip.mp4#t=10")[Video]
]
#document("paper.pdf")[#rect(width: 1pt, height: 1pt) <pdf-part>]
#asset("clip.mp4", "video")
"#,
        )
        .unwrap();
        let references = site_references(&snapshot, &SiteUrlMount::root(), &world);
        assert_eq!(references.references().len(), 2);
        for (reference, path, fragment) in [
            (
                references.references().next().unwrap(),
                "/paper.pdf",
                "pdf-part",
            ),
            (references.references().nth(1).unwrap(), "/clip.mp4", "t=10"),
        ] {
            let ReferenceResolution::Found { target } = reference.resolution() else {
                panic!("expected a resolved media target: {reference:?}");
            };
            assert_eq!(target.url().as_str(), path);
            assert_eq!(target.fragment(), Some(fragment));
        }
    }

    #[test]
    fn base_href_preserves_resolved_target() {
        for (base, href, query, fragment) in [
            ("/docs/", "target/", None, None),
            (
                "/docs/target/?view=base",
                "#intro",
                Some("view=base"),
                Some("intro"),
            ),
        ] {
            let (_directory, snapshot, _graph, world) = candidate(&format!(
                r#"#document("guide/index.html", html.html(
  html.head(html.base(href: "{base}"))
  + html.body(html.a(href: "{href}")[Target])
))
#document("docs/target/index.html")[#html.h1(id: "intro")[Intro]]"#,
            ))
            .unwrap();
            let index = site_references(&snapshot, &SiteUrlMount::root(), &world);
            let ReferenceResolution::Found { target } =
                index.references().next().unwrap().resolution()
            else {
                panic!("{href} did not resolve against {base}");
            };
            assert_eq!(target.url().as_str(), "/docs/target/");
            assert_eq!(target.query(), query);
            assert_eq!(target.fragment(), fragment);
        }
    }

    #[test]
    fn mount_rejects_urls_outside_its_prefix() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/target/")[Outside mount]
  #html.a(href: "/docs/target/")[Mounted]
  #html.a(href: "target/")[Relative]
]
#document("target/index.html")[Target]"#,
        )
        .unwrap();

        let index = site_references(
            &snapshot,
            &SiteUrlMount::from_base_path("/docs/").unwrap(),
            &world,
        );

        assert!(matches!(
            index.references().next().unwrap().resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::OutsideSiteMount { required_prefix, .. }
            } if required_prefix == "/docs/"
        ));
        assert!(matches!(
            index.references().nth(1).unwrap().resolution(),
            ReferenceResolution::Found { .. }
        ));
        assert!(matches!(
            index.references().nth(2).unwrap().resolution(),
            ReferenceResolution::Found { .. }
        ));
    }

    #[test]
    fn destination_rules_differ_per_element() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.object(data: "/embedded/")[]
  #html.img(src: "/embedded/")
]
#document("embedded/index.html")[Embedded document]"#,
        )
        .unwrap();

        let index = site_references(&snapshot, &SiteUrlMount::root(), &world);

        assert_eq!(
            index.references().next().unwrap().category(),
            ReferenceCategory::Resource
        );
        assert!(matches!(
            index.references().next().unwrap().resolution(),
            ReferenceResolution::Found { .. }
        ));
        assert_eq!(
            index.references().nth(1).unwrap().category(),
            ReferenceCategory::Resource
        );
        assert!(matches!(
            index.references().nth(1).unwrap().resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::DocumentNotAllowed { document, .. }
            } if document.as_str() == "/embedded/"
        ));
    }

    #[test]
    fn prefetch_never_becomes_navigation() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html", html.html(html.head(
  html.elem("link", attrs: (rel: "PrEfEtCh prefetch", href: "/docs/next/"))
  + html.link(rel: "prefetch", href: "/docs/download.txt")
  + html.link(rel: "prefetch", href: "/docs/missing/")
)))
#document("next/index.html")[Next]
#asset("download.txt", "Download")"#,
        )
        .unwrap();
        let index = site_references(
            &snapshot,
            &SiteUrlMount::from_base_path("/docs/").unwrap(),
            &world,
        );
        let references = index.references().collect::<Vec<_>>();
        assert_eq!(references.len(), 3);
        for (reference, expected) in references[..2].iter().zip(["/next/", "/download.txt"]) {
            assert_eq!(reference.category(), ReferenceCategory::Resource);
            let ReferenceResolution::Found { target } = reference.resolution() else {
                panic!("prefetch did not resolve: {reference:?}");
            };
            assert_eq!(target.url().as_str(), expected);
        }
        assert_eq!(references[2].category(), ReferenceCategory::Resource);
        assert!(matches!(
            references[2].resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::TargetMissing { .. }
            }
        ));

        // A prefetch relation never relaxes the destination each use already requires.
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html", html.html(html.head(
  html.link(rel: ("stylesheet", "prefetch"), href: "/next/")
  + html.link(rel: ("prefetch", "modulepreload"), href: "/next/")
  + html.link(rel: ("icon", "prefetch"), href: "/next/")
  + html.link(rel: ("preload", "prefetch"), href: "/next/")
  + html.script(src: "/next/")
  + html.script(type: "module", src: "/next/")
) + html.body(html.img(src: "/next/"))))
#document("next/index.html")[Next]"#,
        )
        .unwrap();
        let index = site_references(&snapshot, &SiteUrlMount::root(), &world);
        assert_eq!(index.references().len(), 7);
        for reference in index.references() {
            assert_eq!(reference.category(), ReferenceCategory::Resource);
            assert!(
                matches!(
                    reference.resolution(),
                    ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::DocumentNotAllowed { document, .. }
                    } if document.as_str() == "/next/"
                ),
                "{reference:?}"
            );
        }
    }

    #[test]
    fn typed_uses_report_media_type_mismatches() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "/opaque.bin")
    + html.link(rel: "stylesheet", href: "/module.js")
    + html.script(src: "/site.css")
  )
))
#asset("opaque.bin", "")
#asset("module.js", "")
#asset("site.css", "")"#,
        )
        .unwrap();

        let index = site_references(&snapshot, &SiteUrlMount::root(), &world);
        let failures = index
            .references()
            .map(|reference| match reference.resolution() {
                ReferenceResolution::Unresolved {
                    reason:
                        UnresolvedReferenceReason::MediaTypeMismatch {
                            reference_use,
                            required,
                            declared,
                        },
                } => (*reference_use, required.clone(), declared.clone()),
                resolution => panic!("expected media-type mismatch, got {resolution:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            failures,
            [
                (
                    tola_typst::HtmlReferenceUse::Stylesheet,
                    ResponseMediaType::CSS,
                    ResponseMediaType::OCTET_STREAM,
                ),
                (
                    tola_typst::HtmlReferenceUse::Stylesheet,
                    ResponseMediaType::CSS,
                    ResponseMediaType::JAVASCRIPT,
                ),
                (
                    tola_typst::HtmlReferenceUse::ClassicScript,
                    ResponseMediaType::JAVASCRIPT,
                    ResponseMediaType::CSS,
                ),
            ]
        );
    }

    #[test]
    fn external_uses_need_no_declaration() {
        let (_directory, snapshot, _graph, world) = candidate(
            r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "https://cdn.example/site")
    + html.script(type: "module", src: "//cdn.example/module")
  )
))"#,
        )
        .unwrap();

        let index = site_references(&snapshot, &SiteUrlMount::root(), &world);

        assert_eq!(index.references().len(), 2);
        assert!(index.references().all(|reference| {
            reference.category() == ReferenceCategory::External
                && reference.resolution() == &ReferenceResolution::External
        }));
    }

    #[test]
    fn duplicate_dom_ids_reject_the_candidate() {
        let error = match candidate(
            r#"#document("index.html")[
  #html.div(id: "same")[]
  #html.div(id: "same")[]
]"#,
        ) {
            Err(error) => error,
            Ok(_) => panic!("duplicate final DOM ids must reject the candidate"),
        };

        assert!(matches!(
            error.downcast_ref::<crate::site::index::SiteIndexError>(),
            Some(crate::site::index::SiteIndexError::Registration(
                crate::site::space::AddressRegistrationError::FragmentTargetRepeated {
                    output,
                    fragment,
                }
            )) if output.as_str() == "index.html" && fragment == "same"
        ));
    }
}
