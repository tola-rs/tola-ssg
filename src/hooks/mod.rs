//! Hook system for build automation.
//!
//! This module provides:
//! - `runner`: Hook execution utilities (environment variables, command execution)
//! - `css`: CSS processor integrations (tailwind, etc.)

mod runner;
pub mod css;

pub use runner::*;
