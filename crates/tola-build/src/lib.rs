//! Tola site construction, validated outputs, and reusable build sessions.
//!
//! Command-line handling, filesystem watching, HTTP serving, and editor transport
//! belong to applications using this crate.
//!
//! The optional `json-schema` feature implements `schemars::JsonSchema` for the
//! records in [`diagnostic`], using their Serde serialization contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
// A build session's `Send` proof resolves deeper than rustc's default recursion limit for the lib
// test target (rust-lang/rust#159228).
#![recursion_limit = "256"]

mod asset;
pub mod build;
pub mod cancellation;
pub mod check;
pub mod codes;
mod compiler;
pub mod config;
mod content;
pub mod diagnostic;
pub mod filesystem;
pub mod hooks;
pub mod html;
mod icon;
mod image;
pub mod inspect;
pub mod metadata;
mod mode;
mod observation;
pub mod output;
pub mod package;
mod resources;
mod seo;
pub mod site;

pub use asset::{AssetOrigin, AssetUrls, PublishedAsset};
pub use build::BuildSession;
pub use content::{SourceDescriptorField, SourceDescriptorFieldKind, has_typ_extension};
pub use filesystem::INTERNAL_DIR;
pub use icon::remote_collection_bytes;
pub use resources::{BuildResources, HttpDownloadError, InputScope, NetworkAccess};
pub use seo::feed::FeedFormat;
