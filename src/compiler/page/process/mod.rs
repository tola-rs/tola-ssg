//! Page processing pipeline.
//!
//! - [`batch`] - Batch compilation for full site builds
//! - [`single`] - Single page compilation for watch mode

mod batch;
mod single;

pub use batch::{build_address_space, build_static_pages, populate_pages, rebuild_iterative_pages};
pub use batch::collect_content_files;
pub use single::process_page;
