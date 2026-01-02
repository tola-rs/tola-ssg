//! Page types: metadata, routing, and storage.

mod compiled;
mod kind;
mod links;
mod meta;
mod route;
mod store;

pub use compiled::{CompiledPage, Pages};
pub use kind::PageKind;
pub use links::PAGE_LINKS;
pub use meta::PageMeta;
pub use route::PageRoute;
pub use store::{StoredPage, STORED_PAGES};



/// A JSON object map for storing arbitrary metadata fields.
pub type JsonMap = serde_json::Map<String, serde_json::Value>;

