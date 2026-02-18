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
//! content/posts/hello/      <->   /blog/hello/  (colocated assets)
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
//! use crate::address::GLOBAL_ADDRESS_SPACE;
//!
//! let space = GLOBAL_ADDRESS_SPACE.read();
//!
//! // Resolve any link
//! let result = space.resolve("/about/", &context);
//!
//! // Check if a URL exists
//! if space.contains_url("/posts/hello/") { ... }
//!
//! // Find URL for a source file
//! if let Some(url) = space.url_for_source(&path) { ... }
//! ```

// Allow dead code - infrastructure for hot reload (Phase 5) and link validation
#![allow(dead_code)]

pub mod conflict;
mod resolve;
mod resource;
mod space;

use std::sync::LazyLock;

use parking_lot::RwLock;

// Re-export public types
pub use resolve::{ResolveContext, ResolveResult, resolve_physical_path};
pub use resource::Resource;
pub use space::{AddressSpace, PermalinkUpdate};

/// Global site address space
pub static GLOBAL_ADDRESS_SPACE: LazyLock<RwLock<AddressSpace>> =
    LazyLock::new(|| RwLock::new(AddressSpace::new()));
