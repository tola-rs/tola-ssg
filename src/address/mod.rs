//! Site address space - unified model for all addressable resources.
//!
//! This module provides [`AddressSpace`], a bidirectional mapping between
//! source files and URLs that enables O(1) link validation.
//!
//! # Relationship with [`LinkKind`](crate::core::LinkKind)
//!
//! - [`LinkKind`](crate::core::LinkKind): **Syntactic** classification (fast, no context needed)
//! - [`AddressSpace::resolve`]: **Semantic** resolution (needs site context)
//!
//! # Architecture
//!
//! ```text
//! Source Files                    URL Space
//! ============                   =========
//! content/posts/hello.typ   <->   /blog/hello/
//! content/posts/image.png   <->   /blog/posts/image.png  (content assets)
//! assets/logo.png           <->   /assets/logo.png
//! ```
//!
//! # Module Structure
//!
//! - [`conflict`]: URL conflict detection (multiple sources -> same URL)
//! - [`resource`]: Resource types (Page, Asset, AssetKind)
//! - [`resolve`]: Link resolution types and utilities
//! - [`space`]: AddressSpace core implementation
//!
//! # Usage
//!
//! ```ignore
//! state.read(|_, space| {
//!     // Resolve any link
//!     let result = space.resolve("/about/", &context);
//!
//!     // Check if a URL exists
//!     if space.contains_url("/posts/hello/") { ... }
//!
//!     // Find URL for a source file
//!     if let Some(url) = space.url_for_source(&path) { ... }
//! });
//! ```

// Allow dead code - infrastructure for hot reload (Phase 5) and link validation
#![allow(dead_code)]

pub mod conflict;
mod resolve;
mod resource;
mod space;

use parking_lot::RwLock;

use crate::page::StoredPageMap;

// Re-export public types
pub use resolve::{ResolveContext, ResolveResult, resolve_physical_path};
pub use resource::Resource;
pub use space::{AddressSpace, PermalinkUpdate};

/// Page metadata and address mappings for one site invocation.
#[derive(Debug, Default)]
pub struct SiteIndex {
    /// Gates whole-index publication against page-store readers and writers.
    publish: RwLock<()>,
    pages: StoredPageMap,
    address: RwLock<AddressSpace>,
}

impl SiteIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read<T>(&self, read: impl FnOnce(&StoredPageMap, &AddressSpace) -> T) -> T {
        let _publish = self.publish.read();
        let address = self.address.read();
        read(&self.pages, &address)
    }

    pub fn with_pages<T>(&self, access: impl FnOnce(&StoredPageMap) -> T) -> T {
        let _publish = self.publish.read();
        access(&self.pages)
    }

    pub fn edit<T>(&self, edit: impl FnOnce(&StoredPageMap, &mut AddressSpace) -> T) -> T {
        let _publish = self.publish.write();
        let mut address = self.address.write();
        edit(&self.pages, &mut address)
    }

    pub fn clear(&self) {
        self.edit(|pages, address| {
            pages.clear();
            address.clear();
        });
    }

    /// Replace page and address data as one published snapshot.
    pub fn replace_with(&self, next: Self) {
        let _publish = self.publish.write();
        let SiteIndex { pages, address, .. } = next;
        self.pages.replace_with(pages);
        *self.address.write() = address.into_inner();
    }
}
