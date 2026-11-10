//! References and address reads retained for one output document.

use std::collections::BTreeMap;
use std::sync::Arc;

use tola_address::{OutputPath, SiteUrlMount, UrlPath};

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::site::space::AddressDependencies;
use crate::site::{AddressSpace, HtmlPage};

use super::{Reference, ReferenceResolution, target::read_reference};

/// Source meaning and the exact target reads that justify one immutable group.
pub(super) struct DocumentReferences {
    pub(super) output: OutputPath,
    permalink: UrlPath,
    inventory: Arc<tola_typst::HtmlDocumentInventory>,
    dependencies: Arc<AddressDependencies>,
    pub(super) references: Vec<Reference>,
    pub(super) incoming: BTreeMap<UrlPath, Vec<usize>>,
    /// The document base that names another origin, with its source span, when the page has one.
    ///
    /// Every relative reference of such a page resolves external, so the page's reference checks
    /// are skipped; retaining the base lets the projection report that instead of staying silent.
    pub(super) unchecked_base: Option<UncheckedBase>,
}

/// A document base naming another origin, and where the page wrote it.
#[derive(Clone)]
pub(crate) struct UncheckedBase {
    pub(super) value: String,
    pub(super) origin: Option<tola_diagnostic::Location>,
}

impl DocumentReferences {
    pub(super) fn read(
        cached: Option<&Arc<Self>>,
        address: &AddressSpace,
        page: &HtmlPage,
        inventory: &Arc<tola_typst::HtmlDocumentInventory>,
        url_mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
        cancellation: &BuildCancellation,
    ) -> Result<Arc<Self>, BuildCancelled> {
        if let Some(cached) = cached
            && cached.same_reference_semantics(page, inventory, cancellation)?
            && cached.dependencies.unchanged(address, cancellation)?
        {
            return Ok(
                match cached.refresh_origins(inventory, world, cancellation)? {
                    Some(refreshed) => Arc::new(refreshed),
                    None => Arc::clone(cached),
                },
            );
        }
        Self::read_new(address, page, inventory, url_mount, world, cancellation).map(Arc::new)
    }

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
        let base_moved = unchecked_base.as_ref().map(|base| base.origin.as_ref())
            != self
                .unchecked_base
                .as_ref()
                .map(|base| base.origin.as_ref());
        let refreshed = references.is_some() || base_moved;
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

    fn read_new(
        address: &AddressSpace,
        page: &HtmlPage,
        inventory: &Arc<tola_typst::HtmlDocumentInventory>,
        url_mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
        cancellation: &BuildCancellation,
    ) -> Result<Self, BuildCancelled> {
        let mut references = Vec::with_capacity(inventory.references().len());
        let mut dependencies = AddressDependencies::default();
        let mut incoming = BTreeMap::<UrlPath, Vec<usize>>::new();
        let base = inventory.base_href().map(tola_typst::HtmlBaseHref::value);
        for reference in inventory.references() {
            cancellation.ensure_active()?;
            let resolved = read_reference(
                address,
                page,
                base,
                reference,
                url_mount,
                world,
                &mut dependencies,
            );
            if let ReferenceResolution::Found { target } = resolved.resolution() {
                incoming
                    .entry(target.url().clone())
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
