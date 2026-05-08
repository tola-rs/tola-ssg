//! Page processing pipeline.
//!
//! - [`batch`] - Batch compilation for full site builds
//! - [`single`] - Single page compilation for watch mode

mod batch;
mod single;

pub use batch::collect_content_files;
pub use batch::{
    GlobalStateMode, build_address_space, build_static_pages, populate_pages,
    rebuild_iterative_pages,
};
pub use single::{PageStateEpoch, PageStateTicket, process_page, process_page_with_ticket};
