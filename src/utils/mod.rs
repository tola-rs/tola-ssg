//! General-purpose utilities.
//!
//! This module contains pure, reusable functions with no side effects.
//! If a function is only used by one module, consider moving it there.
//!
//! # Modules
//!
//! - [`css`]: CSS processing (minification)
//! - [`date`]: Date/time formatting (RFC 2822, etc.)
//! - [`exec`]: External command execution
//! - [`git`]: Git repository operations
//! - [`hash`]: Content hashing (BLAKE3)
//! - [`hooks`]: Build hook execution
//! - [`path`]: Path and URL utilities
//! - [`platform`]: Platform-specific helpers
//! - [`plural`]: Pluralization utilities

pub mod css;
pub mod date;
pub mod exec;
pub mod git;
pub mod hash;
pub mod hooks;
pub mod html;
pub mod mime;
pub mod path;
pub mod platform;
pub mod plural;

pub use html::ansi_to_html;
pub use plural::{plural_count, plural_s};
