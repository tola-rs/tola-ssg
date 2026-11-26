//! References resolved from the sealed site output.

pub(crate) mod diagnostic;
mod document;
mod target;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use tola_address::{OutputPath, SiteRef, SiteUrlMount, UrlPath};

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::output::semantics::ResponseMediaType;
use crate::site::SiteIndex;
use document::{DocumentReferences, UncheckedBase};

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

struct ReferenceIter<'a> {
    groups: std::slice::Iter<'a, Arc<DocumentReferences>>,
    current: std::slice::Iter<'a, Reference>,
    remaining: usize,
}

impl<'a> Iterator for ReferenceIter<'a> {
    type Item = &'a Reference;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(ref_) = self.current.next() {
                self.remaining -= 1;
                return Some(ref_);
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceResolution {
    Found { target: ReferenceTarget },
    External,
    Unresolved { reason: UnresolvedReferenceReason },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceTarget {
    url: UrlPath,
    query: Option<String>,
    fragment: Option<String>,
}

impl ReferenceTarget {
    fn new(url: UrlPath, reference: SiteRef) -> Self {
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

    /// Decoded fragment without the leading hash sign.
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
}

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
        suggestion: Option<String>,
    },
    /// Typst publishes element ids only for linked elements; hand-written fragments to
    /// unlinked headings fail unless the element declares an explicit `id`.
    FragmentMissing {
        fragment: String,
        available: Arc<[String]>,
    },
    DocumentNotAllowed {
        reference: String,
        document: UrlPath,
    },
    MediaTypeMismatch {
        reference_use: tola_typst::HtmlReferenceUse,
        required: ResponseMediaType,
        declared: ResponseMediaType,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    page: OutputPath,
    destination: String,
    html_context: String,
    category: ReferenceCategory,
    resolution: ReferenceResolution,
    origin: Option<tola_diagnostic::Location>,
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

    pub fn origin(&self) -> Option<&tola_diagnostic::Location> {
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
        index: &SiteIndex,
        url_mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
        cancellation: &BuildCancellation,
        previous: Option<&References>,
    ) -> Result<Self, BuildCancelled> {
        cancellation.ensure_active()?;
        let prev = previous.filter(|p| &p.0.url_mount == url_mount);
        let mut changed_groups = None;
        let mut page_count = 0;
        for (page, inv) in index.html_pages() {
            cancellation.ensure_active()?;
            let cached =
                prev.and_then(|p| p.0.outgoing.get(&page.output).map(|&pos| &p.0.groups[pos]));
            let group = DocumentReferences::read(
                cached,
                index.address(),
                page,
                inv,
                url_mount,
                world,
                cancellation,
            )?;
            let unchanged_prefix = prev
                .and_then(|p| p.0.groups.get(page_count))
                .is_some_and(|at| Arc::ptr_eq(at, &group));
            if changed_groups.is_some() || !unchanged_prefix {
                let groups = changed_groups.get_or_insert_with(|| {
                    prev.map_or_else(Vec::new, |p| p.0.groups[..page_count].to_vec())
                });
                groups.push(group);
            }
            page_count += 1;
        }
        cancellation.ensure_active()?;
        let groups = match changed_groups {
            Some(groups) => groups,
            None => match prev {
                Some(p) if p.0.groups.len() == page_count => return Ok(p.clone()),
                Some(p) => p.0.groups[..page_count].to_vec(),
                None => Vec::new(),
            },
        };
        let mut incoming = BTreeMap::<UrlPath, Vec<usize>>::new();
        let mut outgoing = BTreeMap::new();
        let mut len = 0;
        for (pos, group) in groups.iter().enumerate() {
            cancellation.ensure_active()?;
            outgoing.insert(group.output.clone(), pos);
            len += group.references.len();
            for url in group.incoming.keys() {
                cancellation.ensure_active()?;
                incoming.entry(url.clone()).or_default().push(pos);
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

    pub(crate) fn unchecked_bases(&self) -> impl Iterator<Item = (&OutputPath, &UncheckedBase)> {
        self.0
            .groups
            .iter()
            .filter_map(|g| g.unchecked_base.as_ref().map(|b| (&g.output, b)))
    }

    /// In page and DOM order.
    pub fn incoming(&self, url: &UrlPath) -> impl Iterator<Item = &Reference> {
        self.0
            .incoming
            .get_key_value(url)
            .into_iter()
            .flat_map(move |(url, groups)| {
                groups.iter().flat_map(move |&pos| {
                    let group = &self.0.groups[pos];
                    group.incoming[url]
                        .iter()
                        .map(move |&idx| &group.references[idx])
                })
            })
    }

    /// Includes unresolved references.
    pub fn outgoing(&self, page: &OutputPath) -> impl Iterator<Item = &Reference> {
        self.0
            .outgoing
            .get(page)
            .into_iter()
            .flat_map(|&pos| self.0.groups[pos].references.iter())
    }
}
