//! Page types: metadata, routing, and storage.

mod compiled;
mod iteration;
mod kind;
mod links;
mod meta;
mod resolve;
mod route;
mod state;
mod store;

pub use compiled::{CompiledPage, Pages};
pub use iteration::{HashStabilityTracker, StabilityDecision};
pub use kind::PageKind;
pub use links::PAGE_LINKS;
pub use meta::PageMeta;
pub use resolve::resolve_page_link_target;
pub use route::PageRoute;
pub use state::{PageState, StaleLinkPolicy};
pub use store::{STORED_PAGES, StoredPage, StoredPageMap};

/// A JSON object map for storing arbitrary metadata fields
pub type JsonMap = serde_json::Map<String, serde_json::Value>;
