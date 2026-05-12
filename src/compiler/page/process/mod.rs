//! Page processing pipeline.
//!
//! - [`batch`] - Batch compilation for full site builds
//! - [`single`] - Single page compilation for watch mode

mod batch;
mod single;

use crate::compiler::FeedBodyMode;
use crate::config::SiteConfig;

pub use batch::collect_content_files;
pub use batch::{
    GlobalStateMode, build_address_space, build_static_pages, populate_pages,
    rebuild_iterative_pages,
};
pub use single::{PageStateEpoch, PageStateTicket};
pub(crate) use single::{PreparedPage, commit_page_state_parts, prepare_page};

fn feed_body_mode(config: &SiteConfig) -> FeedBodyMode {
    if config.site.seo.needs_feed_body() {
        FeedBodyMode::Render
    } else {
        FeedBodyMode::Skip
    }
}
