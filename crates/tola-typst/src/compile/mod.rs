//! Compiling one prepared world into an HTML document or an eager evaluation.
//!
//! One world, one outcome:
//!
//! - [`compile_world`] - compile a world to an HTML document
//! - `scan_world` - evaluate a module without layout (the `scan` feature)
//!
//! Bundle realization and export live in [`crate::bundle`]; compiling many independent
//! worlds at once is the caller's scheduling decision.

pub mod html;
#[cfg(feature = "scan")]
pub mod scan;

pub use html::{
    CompileFailure, CompileResult, compile_source_with_evidence, compile_world,
    compile_world_with_evidence,
};

#[cfg(feature = "scan")]
pub use scan::{
    CapturedValues, ObservedScan, ScanFailure, ScanResult, scan_world, scan_world_observed,
    scan_world_with_evidence,
};
