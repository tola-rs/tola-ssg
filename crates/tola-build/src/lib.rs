//! Tola site construction, validated outputs, and reusable build sessions.
//!
//! Command-line handling, filesystem watching, HTTP serving, and editor transport
//! belong to applications using this crate.
//!
//! Diagnostics are [`tola_diagnostic`]'s records; the optional `json-schema` feature forwards to
//! that crate's schema derivation.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
// A build session's `Send` proof resolves deeper than rustc's default recursion limit for the lib
// test target (rust-lang/rust#159228).
#![recursion_limit = "256"]

mod asset;
pub mod build;
pub mod cancellation;
pub mod check;
mod compiler;
pub mod config;
mod content;
pub mod filesystem;
pub mod hooks;
pub mod html;
mod icon;
mod image;
pub mod inputs;
pub mod inspect;
pub mod metadata;
mod mode;
pub mod modules;
mod observation;
pub mod output;
pub mod package;
pub mod report;
mod resources;
mod seo;
pub mod site;

pub use asset::{AssetOrigin, AssetUrls, ModuleAsset, ModuleAssetInput, PublishedAsset};
pub use build::BuildSession;
pub use compiler::diagnostic::font_failure;
pub use content::{
    ContentDiscoveryError, SourceDescriptorField, SourceDescriptorFieldKind, has_typ_extension,
};
pub use icon::{IconError, IconIntegrityError, refresh_icon_collections, remote_collection_bytes};
pub use resources::{BuildResources, HttpDownloadError};
pub use seo::feed::FeedFormat;
