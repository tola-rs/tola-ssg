//! Core types - pure abstractions shared across the codebase.

mod category;
mod driver;
mod link;
mod priority;
mod state;
mod url;

pub use crate::address::{GLOBAL_ADDRESS_SPACE, ResolveContext, ResolveResult};

pub use category::{ContentKind, FileCategory};
pub use driver::BuildMode;
pub use link::LinkKind;
pub use priority::Priority;
pub use state::{
    is_healthy, is_serving, is_shutdown, register_server, set_healthy, set_serving,
    setup_shutdown_handler,
};
pub use url::{UrlChange, UrlPath};
