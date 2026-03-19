//! Format adapter interfaces and pre-scan data.

mod adapter;
mod batch;
mod scanned;
mod single;

pub use adapter::PageFormat;
pub use batch::scan_pages;
pub use scanned::{ScannedHeading, ScannedPage, ScannedPageLink};
pub use single::scan_single_page;
