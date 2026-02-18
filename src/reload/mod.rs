//! Reload Module
//!
//! Provides WebSocket-based live reload for development.
//!
//! # Architecture
//!
//! The reload system is built on the Actor model:
//!
//! ```text
//! FsActor -> CompilerActor -> VdomActor -> WsActor -> Browser
//!   (watch)    (typst)       (diff)    (broadcast)
//! ```
//!
//! # Modules
//!
//! - `active` - Active page tracking for prioritized compilation
//! - `classify` - File categorization and dependency resolution
//! - `compile` - Typst to VDOM compilation
//! - `diff` - VDOM diffing for incremental updates
//! - `message` - Hot reload message types (reload, patch, css)
//! - `patch` - DOM patch operations for incremental updates
//! - `queue` - Compile queue for prioritized compilation
//! - `server` - WebSocket server for client connections

pub mod active;
pub mod classify;
pub mod compile;
pub mod diff;
pub mod message;
pub mod patch;
pub mod queue;
pub mod server;
