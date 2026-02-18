//! Patch Operations
//!
//! DOM patch operations for incremental hot reload updates.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use tola_vdom::algo::{Anchor, Patch};

// =============================================================================
// Patch Operation
// =============================================================================

/// Individual patch operation for DOM updates (anchor-based)
///
/// All operations use StableId for targeting. No position indices
/// This design ensures order independence and prevents index drift
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum PatchOp {
    /// Replace entire element's outerHTML
    Replace {
        /// StableId (hex) of element to replace
        target: String,
        /// New HTML content
        html: String,
    },

    /// Update text content (element.textContent = text)
    /// Used for single-text-child elements: `<p>Hello</p>` -> `<p>World</p>`
    Text {
        /// StableId (hex) of element
        target: String,
        /// New text content (plain text, will be escaped by textContent)
        text: String,
    },

    /// Replace inner HTML (element.innerHTML = html)
    /// Used when child structure changes but parent element preserved
    Html {
        /// StableId (hex) of element
        target: String,
        /// New innerHTML content
        html: String,
        /// Whether content is SVG (needs SVG namespace parsing)
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_svg: bool,
    },

    /// Remove element by ID
    Remove {
        /// StableId (hex) of element to remove
        target: String,
    },

    /// Insert new content at anchor position
    Insert {
        /// Anchor type: "after", "before", "first", "last"
        anchor_type: String,
        /// StableId (hex) of anchor element
        anchor_id: String,
        /// HTML content to insert
        html: String,
    },

    /// Move element to new anchor position
    Move {
        /// StableId (hex) of element to move
        target: String,
        /// Anchor type: "after", "before", "first", "last"
        anchor_type: String,
        /// StableId (hex) of anchor element
        anchor_id: String,
    },

    /// Update element attributes
    Attrs {
        /// StableId (hex) of element
        target: String,
        /// Attributes to set (None = remove attribute)
        attrs: Vec<(String, Option<String>)>,
    },
}

// =============================================================================
// Constructors
// =============================================================================

impl PatchOp {
    /// Create a replace operation
    pub fn replace(target: impl Into<String>, html: impl Into<String>) -> Self {
        Self::Replace {
            target: target.into(),
            html: html.into(),
        }
    }

    /// Create a text update operation
    pub fn text(target: impl Into<String>, text: impl Into<String>) -> Self {
        Self::Text {
            target: target.into(),
            text: text.into(),
        }
    }

    /// Create a remove operation
    pub fn remove(target: impl Into<String>) -> Self {
        Self::Remove {
            target: target.into(),
        }
    }

    /// Create an insert-after operation
    pub fn insert_after(anchor_id: impl Into<String>, html: impl Into<String>) -> Self {
        Self::Insert {
            anchor_type: "after".to_string(),
            anchor_id: anchor_id.into(),
            html: html.into(),
        }
    }

    /// Create an insert-first-child operation
    pub fn insert_first(parent_id: impl Into<String>, html: impl Into<String>) -> Self {
        Self::Insert {
            anchor_type: "first".to_string(),
            anchor_id: parent_id.into(),
            html: html.into(),
        }
    }

    /// Create an attribute update operation
    pub fn attrs(target: impl Into<String>, attrs: Vec<(String, Option<String>)>) -> Self {
        Self::Attrs {
            target: target.into(),
            attrs,
        }
    }
}

// =============================================================================
// Conversion from VDOM Patches
// =============================================================================

/// Convert VDOM patches to PatchOps for WebSocket transmission
pub fn from_vdom_patches(patches: &[Patch]) -> Vec<PatchOp> {
    patches.iter().map(patch_to_op).collect()
}

fn patch_to_op(patch: &Patch) -> PatchOp {
    match patch {
        Patch::Replace { target, html } => PatchOp::Replace {
            target: target.to_attr_value(),
            html: html.clone(),
        },
        Patch::UpdateText { target, text } => PatchOp::Text {
            target: target.to_attr_value(),
            text: text.clone(),
        },
        Patch::ReplaceChildren {
            target,
            html,
            is_svg,
        } => PatchOp::Html {
            target: target.to_attr_value(),
            html: html.clone(),
            is_svg: *is_svg,
        },
        Patch::Remove { target } => PatchOp::Remove {
            target: target.to_attr_value(),
        },
        Patch::Insert { anchor, html } => {
            let (anchor_type, anchor_id) = anchor_to_parts(anchor);
            PatchOp::Insert {
                anchor_type,
                anchor_id,
                html: html.clone(),
            }
        }
        Patch::Move { target, to } => {
            let (anchor_type, anchor_id) = anchor_to_parts(to);
            PatchOp::Move {
                target: target.to_attr_value(),
                anchor_type,
                anchor_id,
            }
        }
        Patch::UpdateAttrs { target, attrs } => PatchOp::Attrs {
            target: target.to_attr_value(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_ref().map(|s| s.to_string())))
                .collect(),
        },
    }
}

fn anchor_to_parts(anchor: &Anchor) -> (String, String) {
    match anchor {
        Anchor::After(id) => ("after".to_string(), id.to_attr_value()),
        Anchor::Before(id) => ("before".to_string(), id.to_attr_value()),
        Anchor::FirstChildOf(id) => ("first".to_string(), id.to_attr_value()),
        Anchor::LastChildOf(id) => ("last".to_string(), id.to_attr_value()),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tola_vdom::id::StableId;

    #[test]
    fn test_anchor_based_insert() {
        let anchor_id = StableId::for_text(0, 0x1234);

        let patches = vec![Patch::Insert {
            anchor: Anchor::After(anchor_id),
            html: "<span>new</span>".to_string(),
        }];

        let ops = from_vdom_patches(&patches);
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
    }

    #[test]
    fn test_anchor_based_move() {
        let target_id = StableId::for_text(1, 0x1111);
        let anchor_id = StableId::for_text(2, 0x2222);

        let patches = vec![Patch::Move {
            target: target_id,
            to: Anchor::FirstChildOf(anchor_id),
        }];

        let ops = from_vdom_patches(&patches);
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
    }
}
