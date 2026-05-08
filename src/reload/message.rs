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
pub use crate::reload::patch::ClientPatch;

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
        ops: Vec<ClientPatch>,
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

    /// Keep-alive ping (server -> client)
    Ping {
        /// Timestamp for latency measurement
        ts: u64,
    },

    /// Keep-alive pong (client -> server)
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

    /// Clear one error or the full error overlay state
    #[serde(rename = "clear_error")]
    ClearError {
        /// Optional source file path to clear.
        /// If omitted, the client clears all tracked errors before replay.
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
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
    pub fn patch(path: impl Into<String>, ops: Vec<ClientPatch>) -> Self {
        Self::Patch {
            path: path.into(),
            ops,
            url_change: None,
        }
    }

    /// Create a patch message with URL change
    pub fn patch_with_url_change(
        path: impl Into<String>,
        ops: Vec<ClientPatch>,
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

    /// Create a clear error message for one source file
    pub fn clear_error(path: impl Into<String>) -> Self {
        Self::ClearError {
            path: Some(path.into()),
        }
    }

    /// Create a clear-all-errors message
    pub const fn clear_all_errors() -> Self {
        Self::ClearError { path: None }
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

    /// Create a patch message from rendered patches (anchor-based)
    ///
    /// All operations use StableId for targeting. No position indices.
    /// Order of operations doesn't matter for correctness.
    pub fn from_patches(path: &str, patches: &[tola_vdom::patch::Patch]) -> Self {
        Self::Patch {
            path: path.to_string(),
            ops: crate::reload::patch::from_render_patches(patches),
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
                ClientPatch::replace("abc123", "<p>New content</p>"),
                ClientPatch::text("def456", "Updated Title"),
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
    fn test_clear_error_message_with_path() {
        let msg = HotReloadMessage::clear_error("content/index.typ");
        let json = msg.to_json();
        assert!(json.contains(r#""type":"clear_error""#));
        assert!(json.contains(r#""path":"content/index.typ""#));

        let parsed = HotReloadMessage::from_json(&json).unwrap();
        match parsed {
            HotReloadMessage::ClearError { path } => {
                assert_eq!(path.as_deref(), Some("content/index.typ"));
            }
            _ => panic!("Expected ClearError message"),
        }
    }

    #[test]
    fn test_clear_all_errors_message_omits_path() {
        let msg = HotReloadMessage::clear_all_errors();
        let json = msg.to_json();
        assert_eq!(json, r#"{"type":"clear_error"}"#);

        let parsed = HotReloadMessage::from_json(&json).unwrap();
        match parsed {
            HotReloadMessage::ClearError { path } => {
                assert!(path.is_none());
            }
            _ => panic!("Expected ClearError message"),
        }
    }

    #[test]
    fn hotreload_js_handles_every_serialized_message_type() {
        fn serialized_type(message: HotReloadMessage) -> String {
            let json = message.to_json();
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            value["type"].as_str().unwrap().to_string()
        }

        let runtime = include_str!("../embed/serve/hotreload.js");
        let messages = [
            HotReloadMessage::reload(),
            HotReloadMessage::patch("/index.html", vec![]),
            HotReloadMessage::Css {
                target: "style[data-tola-css-target=\"main\"]".to_string(),
                content: "body { color: red; }".to_string(),
            },
            HotReloadMessage::Ping { ts: 1 },
            HotReloadMessage::Pong { ts: 1 },
            HotReloadMessage::connected(),
            HotReloadMessage::error("content/index.typ", "compile error"),
            HotReloadMessage::clear_all_errors(),
        ];

        for message in messages {
            let ty = serialized_type(message);
            let expected_case = format!("case '{}':", ty);
            assert!(
                runtime.contains(&expected_case),
                "hotreload.js does not handle serialized message type: {ty}"
            );
        }
    }

    #[test]
    fn test_anchor_based_insert() {
        use tola_vdom::diff::Anchor;
        use tola_vdom::identity::StableId;
        use tola_vdom::patch::Patch;

        // Create test StableId using public API
        let anchor_id = StableId::for_text(0, 0x1234);

        let patches = vec![Patch::Insert {
            anchor: Anchor::After(anchor_id),
            html: "<span>new</span>".to_string(),
        }];

        let msg = HotReloadMessage::from_patches("/test.html", &patches);
        if let HotReloadMessage::Patch { ops, .. } = msg {
            assert_eq!(ops.len(), 1);
            if let ClientPatch::Insert {
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
        use tola_vdom::diff::Anchor;
        use tola_vdom::identity::StableId;
        use tola_vdom::patch::Patch;

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
            if let ClientPatch::Move {
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
