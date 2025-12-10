//! Site-wide data management for Typst templates.
//!
//! This module provides a global data store that collects metadata from all pages
//! and exposes it to Typst templates via virtual files (`/_data/*.json`).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         Two-Phase Compilation                           │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                         │
//! │  Phase 1: Metadata Collection                                           │
//! │  ┌─────────────┐     ┌─────────────┐     ┌─────────────────────────┐    │
//! │  │ compile()   │ ──► │ extract     │ ──► │ GLOBAL_SITE_DATA.insert │    │
//! │  │ (all pages) │     │ <tola-meta> │     │ (collect metadata)      │    │
//! │  └─────────────┘     └─────────────┘     └─────────────────────────┘    │
//! │        │                                                                │
//! │        └── json("/_data/...") returns empty/partial data                │
//! │            (HTML discarded - incomplete data)                           │
//! │                                                                         │
//! │  Phase 2: HTML Generation                                               │
//! │  ┌─────────────┐     ┌─────────────────────────┐     ┌─────────────┐    │
//! │  │ compile()   │ ──► │ GLOBAL_SITE_DATA.read   │ ──► │ Write HTML  │    │
//! │  │ (all pages) │     │ (complete data)         │     │ to disk     │    │
//! │  └─────────────┘     └─────────────────────────┘     └─────────────┘    │
//! │        │                                                                │
//! │        └── json("/_data/...") returns complete data                     │
//! │                                                                         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Virtual Files
//!
//! The following virtual files are available to Typst templates:
//!
//! | Path | Description |
//! |------|-------------|
//! | `/_data/pages.json` | All pages with metadata |
//! | `/_data/tags.json` | Pages grouped by tag |
//!
//! # Usage in Typst
//!
//! ```typst
//! #let pages = json("/_data/pages.json")
//! #let tags = json("/_data/tags.json")
//!
//! // List all posts
//! #for page in pages.sorted(key: p => p.date).rev() {
//!     [- #link(page.url)[#page.title]]
//! }
//!
//! // List posts by tag
//! #for (tag, posts) in tags {
//!     [== #tag]
//!     #for post in posts { [- #link(post.url)[#post.title]] }
//! }
//! ```

mod store;
mod types;
pub mod virtual_fs;

pub use store::GLOBAL_SITE_DATA;
pub use types::PageData;
pub use virtual_fs::{is_virtual_data_path, read_virtual_data};
