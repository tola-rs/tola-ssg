//! Hot Reload Message Protocol
//!
//! Defines the JSON message format for WebSocket communication between
//! the development server and browser clients.
//!
//! # Message Types
//!
//! - `reload`: Trigger full page reload
//! - `patch`: Apply incremental DOM patches (with optional URL change)
//! - `css`: Inject updated CSS (no layout recalc)
//! - `ping`/`pong`: Keep connection alive

// Many methods are not yet used but will be for incremental hot reload
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub use crate::core::UrlChange;
pub use crate::reload::patch::PatchOp;

/// Hot reload message sent over WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HotReloadMessage {
    /// Full page reload (fallback when diff is too complex)
    Reload {
        /// Optional reason for reload
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Optional URL change (browser updates URL before reload)
        #[serde(skip_serializing_if = "Option::is_none")]
        url_change: Option<UrlChange>,
    },

    /// Incremental DOM patch with optional URL change
    Patch {
        /// Target page path (e.g., "/blog/post.html")
        path: String,
        /// Sequence of patch operations
        ops: Vec<PatchOp>,
        /// Optional URL change (permalink changed)
        #[serde(skip_serializing_if = "Option::is_none")]
        url_change: Option<UrlChange>,
    },

    /// CSS-only update (fast path - no layout recalc)
    Css {
        /// CSS selector or stylesheet href
        target: String,
        /// New CSS content
        content: String,
    },

    /// Keep-alive ping (server → client)
    Ping {
        /// Timestamp for latency measurement
        ts: u64,
    },

    /// Keep-alive pong (client → server)
    Pong {
        /// Echo back the timestamp
        ts: u64,
    },

    /// Connection established
    Connected {
        /// Server version for compatibility check
        version: String,
    },

    /// Compilation error (display overlay, no reload)
    Error {
        /// Source file path
        path: String,
        /// Error message
        error: String,
    },

    /// Clear error overlay (compilation succeeded after error)
    #[serde(rename = "clear_error")]
    ClearError,
}

impl HotReloadMessage {
    /// Create a reload message
    pub fn reload() -> Self {
        Self::Reload {
            reason: None,
            url_change: None,
        }
    }

    /// Create a reload message with reason
    pub fn reload_with_reason(reason: impl Into<String>) -> Self {
        Self::Reload {
            reason: Some(reason.into()),
            url_change: None,
        }
    }

    /// Create a reload message with URL change
    pub fn reload_with_url_change(reason: impl Into<String>, url_change: UrlChange) -> Self {
        Self::Reload {
            reason: Some(reason.into()),
            url_change: Some(url_change),
        }
    }

    /// Create a patch message
    pub fn patch(path: impl Into<String>, ops: Vec<PatchOp>) -> Self {
        Self::Patch {
            path: path.into(),
            ops,
            url_change: None,
        }
    }

    /// Create a patch message with URL change
    pub fn patch_with_url_change(
        path: impl Into<String>,
        ops: Vec<PatchOp>,
        url_change: UrlChange,
    ) -> Self {
        Self::Patch {
            path: path.into(),
            ops,
            url_change: Some(url_change),
        }
    }

    /// Create a connected message
    pub fn connected() -> Self {
        Self::Connected {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Create an error message (for compilation errors)
    pub fn error(path: impl Into<String>, error: impl Into<String>) -> Self {
        Self::Error {
            path: path.into(),
            error: error.into(),
        }
    }

    /// Create a clear error message
    pub fn clear_error() -> Self {
        Self::ClearError
    }

    /// Create a ping message
    pub fn ping() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self::Ping { ts }
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"type":"reload"}"#.to_string())
    }

    /// Parse from JSON string
    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }

    /// Create a patch message from VDOM Patches (anchor-based)
    ///
    /// All operations use StableId for targeting. No position indices.
    /// Order of operations doesn't matter for correctness.
    pub fn from_patches(path: &str, patches: &[tola_vdom::algo::Patch]) -> Self {
        Self::Patch {
            path: path.to_string(),
            ops: crate::reload::patch::from_vdom_patches(patches),
            url_change: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let msg = HotReloadMessage::patch(
            "/index.html",
            vec![
                PatchOp::replace("abc123", "<p>New content</p>"),
                PatchOp::text("def456", "Updated Title"),
            ],
        );

        let json = msg.to_json();
        assert!(json.contains(r#""type":"patch""#));
        assert!(json.contains(r#""path":"/index.html""#));

        let parsed = HotReloadMessage::from_json(&json).unwrap();
        match parsed {
            HotReloadMessage::Patch { path, ops, .. } => {
                assert_eq!(path, "/index.html");
                assert_eq!(ops.len(), 2);
            }
            _ => panic!("Expected Patch message"),
        }
    }

    #[test]
    fn test_reload_message() {
        let msg = HotReloadMessage::reload_with_reason("template changed");
        let json = msg.to_json();
        assert!(json.contains(r#""type":"reload""#));
        assert!(json.contains(r#""reason":"template changed""#));
    }

    #[test]
    fn test_anchor_based_insert() {
        use tola_vdom::algo::{Anchor, Patch};
        use tola_vdom::id::StableId;

        // Create test StableId using public API
        let anchor_id = StableId::for_text(0, 0x1234);

        let patches = vec![Patch::Insert {
            anchor: Anchor::After(anchor_id),
            html: "<span>new</span>".to_string(),
        }];

        let msg = HotReloadMessage::from_patches("/test.html", &patches);
        if let HotReloadMessage::Patch { ops, .. } = msg {
            assert_eq!(ops.len(), 1);
            if let PatchOp::Insert {
                anchor_type,
                anchor_id: id,
                ..
            } = &ops[0]
            {
                assert_eq!(anchor_type, "after");
                assert_eq!(id, &anchor_id.to_attr_value());
            } else {
                panic!("Expected Insert op");
            }
        } else {
            panic!("Expected Patch message");
        }
    }

    #[test]
    fn test_anchor_based_move() {
        use tola_vdom::algo::{Anchor, Patch};
        use tola_vdom::id::StableId;

        // Create test StableIds using public API
        let target_id = StableId::for_text(1, 0x1111);
        let anchor_id = StableId::for_text(2, 0x2222);

        let patches = vec![Patch::Move {
            target: target_id,
            to: Anchor::FirstChildOf(anchor_id),
        }];

        let msg = HotReloadMessage::from_patches("/test.html", &patches);
        if let HotReloadMessage::Patch { ops, .. } = msg {
            assert_eq!(ops.len(), 1);
            if let PatchOp::Move {
                target,
                anchor_type,
                anchor_id: id,
            } = &ops[0]
            {
                assert_eq!(target, &target_id.to_attr_value());
                assert_eq!(anchor_type, "first");
                assert_eq!(id, &anchor_id.to_attr_value());
            } else {
                panic!("Expected Move op");
            }
        } else {
            panic!("Expected Patch message");
        }
    }
}
