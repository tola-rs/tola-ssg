//! The Typst world and everything it reads.
//!
//! [`TypstWorld`] is what one compilation reads; [`WorldBuilder`] states that
//! configuration. The inputs it draws on live beside it: [`mod@file`] resolves paths
//! and records provenance, [`font`] prepares fonts, [`package`] selects package
//! roots, and [`library`] has `sys.inputs`.

pub mod file;
pub mod font;
pub mod library;
pub mod package;

mod boundary;
mod builder;
mod core;
mod path;
mod snapshot;
mod strategy;

pub use boundary::{SourceBoundary, SourceRefusal};
pub use builder::{WorldBuildError, WorldBuilder};
pub use core::TypstWorld;
pub use path::normalize_path;
pub use snapshot::{SnapshotError, SnapshotLoad, SnapshotLoadFailure, SourceSnapshot};
