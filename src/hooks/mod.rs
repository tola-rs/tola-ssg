//! Hook system for build automation.
//!
//! This module provides:
//! - `runner`: Hook execution utilities (environment variables, command execution)
//! - `css`: CSS processor integrations (tailwind, etc.)

pub mod css;
mod runner;

pub use runner::*;
