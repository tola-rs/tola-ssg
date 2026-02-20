//! Actor Message Definitions
//!
//! Message types for inter-actor communication.
//!
//! ```text
//! FsActor --Compile--> CompilerActor --Process--> VdomActor --Patch--> WsActor
//! ```

use std::path::PathBuf;

use crate::address::PermalinkUpdate;
use crate::compiler::family::Indexed;
use crate::core::{UrlChange, UrlPath};
use crate::reload::queue::CompileQueue;
use tola_vdom::{Document, algo::Patch};

// =============================================================================
// CompilerActor Messages
// =============================================================================

/// Messages to Compiler Actor
#[derive(Debug)]
pub enum CompilerMsg {
    /// Compile files with priority information
    Compile {
        queue: CompileQueue,
        /// Paths that triggered this compile (for running watched hooks)
        changed_paths: Vec<PathBuf>,
    },
    /// Compile content files that depend on changed deps
    #[allow(dead_code)] // Reserved for future dependency-aware rebuild
    CompileDependents(Vec<PathBuf>),
    /// Process asset changes (copy files, trigger reload)
    AssetChange(Vec<PathBuf>),
    /// Full rebuild (config changed)
    FullRebuild,
    /// Shutdown
    #[allow(dead_code)] // Reserved for graceful shutdown
    Shutdown,
}

// =============================================================================
// VdomActor Messages
// =============================================================================

/// Messages to VDOM Actor
#[derive(Debug)]
pub enum VdomMsg {
    /// Process compiled VDOM
    Process {
        path: PathBuf,
        url_path: UrlPath,
        /// Boxed to reduce enum size (Document<Indexed> is ~520 bytes)
        vdom: Box<Document<Indexed>>,
        /// Permalink change detected by CompilerActor (None = unchanged)
        permalink_change: Option<PermalinkUpdate>,
        /// Compilation warnings (for persistence)
        warnings: Vec<String>,
    },
    /// Trigger reload
    Reload { reason: String },
    /// Compilation error (display via VdomActor's WatchStatus for proper overwrite)
    Error {
        path: PathBuf,
        url_path: UrlPath,
        error: String,
    },
    /// File skipped
    Skip,
    /// End of a compilation batch - trigger aggregated log output
    BatchEnd,
    /// Clear cache
    Clear,
    /// Shutdown
    Shutdown,
}

// =============================================================================
// WsActor Messages
// =============================================================================

/// Messages to WebSocket Actor
pub enum WsMsg {
    /// Send patches with optional URL change
    Patch {
        url_path: UrlPath,
        patches: Vec<Patch>,
        /// If set, browser updates URL bar without reload
        url_change: Option<UrlChange>,
    },
    /// Reload page
    Reload {
        reason: String,
        /// - Some: targeted reload (only clients viewing this route)
        /// - None: broadcast reload (all clients)
        url_path: Option<UrlPath>,
        /// If set, browser updates URL before reload
        url_change: Option<UrlChange>,
    },
    /// Compilation error (display overlay, no reload)
    Error { path: String, error: String },
    /// Clear error overlay (compilation succeeded after error)
    ClearError,
    /// Add client
    AddClient(std::net::TcpStream),
    /// Client connected notification
    #[allow(dead_code)] // Reserved for connection tracking
    ClientConnected,
    /// Shutdown
    #[allow(dead_code)] // Reserved for graceful shutdown
    Shutdown,
}
