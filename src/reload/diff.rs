//! Diff Pipeline - VDOM Diffing and Patch Generation
//!
//! Contains both pure and effectful functions for computing VDOM diffs.
//!
//! # Function Types
//!
//! - `diff_vdom()` - Pure function, computes diff without side effects
//! - `compute_diff()` - Effectful, updates cache (used by VdomActor)
//! - `compute_diff_shared()` - Thread-safe version using `SharedCache`

use crate::compiler::family::{
    Cache, CacheEntry, DiffEdit, Indexed, SharedCache, StructuralDocument,
};
use tola_vdom::diff::{Outcome, diff};
use tola_vdom::{CacheKey, Document};

/// Outcome of diff computation
#[derive(Debug)]
pub enum DiffOutcome {
    /// First time seeing this page, no diff possible
    Initial,
    /// No changes detected
    Unchanged,
    /// Tree edits to render and apply.
    /// Includes the new VDOM for cache update after successful broadcast.
    /// Document is boxed to reduce enum size.
    Edits(Vec<DiffEdit>, Box<Document<Indexed>>),
    /// Structural change requires full reload
    NeedsReload { reason: String },
}

// =============================================================================
// Pure Function (no side effects)
// =============================================================================

/// Compute diff between a cached structural VDOM and a freshly indexed VDOM.
///
/// Returns diff result without modifying any state.
pub fn diff_vdom(old_vdom: &StructuralDocument, new_vdom: &Document<Indexed>) -> DiffOutcome {
    match diff(old_vdom, new_vdom) {
        Outcome::Unchanged { .. } => DiffOutcome::Unchanged,
        Outcome::Changed(changes) => DiffOutcome::Edits(changes.edits, Box::new(new_vdom.clone())),
        Outcome::Reload(reload) => DiffOutcome::NeedsReload {
            reason: reload.reason.to_string(),
        },
    }
}

// =============================================================================
// Effectful Function (modifies cache)
// =============================================================================

fn cache_entry_from_indexed(doc: &Document<Indexed>) -> CacheEntry {
    CacheEntry::with_default_version(tola_vdom::snapshot::project(doc))
}

/// Compute diff and update cache appropriately
///
/// # Side Effects
/// - Reads from cache
/// - Writes to cache (for Initial/Reload/Unchanged cases)
///
/// # Cache Update Strategy
/// - Initial: Insert new VDOM (browser will reload anyway)
/// - NeedsReload: Insert new VDOM (browser will reload anyway)
/// - Unchanged: Insert new VDOM (safe, content identical)
/// - Edits: DON'T update cache here - caller updates after successful broadcast
///
/// # Note
/// Caller must create `CacheKey` explicitly to ensure URL normalization
#[allow(dead_code)]
pub fn compute_diff(cache: &mut Cache, key: CacheKey, new_vdom: Document<Indexed>) -> DiffOutcome {
    if let Some(old_entry) = cache.get(&key) {
        let outcome = diff_vdom(&old_entry.doc, &new_vdom);

        match &outcome {
            DiffOutcome::NeedsReload { .. } | DiffOutcome::Unchanged => {
                // Safe to update cache - browser will reload or content is same
                cache.insert(key, cache_entry_from_indexed(&new_vdom));
            }
            DiffOutcome::Edits(..) => {
                // DON'T update cache - caller updates after successful broadcast
                // This keeps cache in sync with what browser actually displays
            }
            DiffOutcome::Initial => unreachable!("old_vdom exists"),
        }

        outcome
    } else {
        // Initial - insert into cache
        cache.insert(key, cache_entry_from_indexed(&new_vdom));
        DiffOutcome::Initial
    }
}

/// Thread-safe version of `compute_diff` using `SharedCache`
///
/// This version is suitable for concurrent access from multiple threads
/// Uses `RwLock` internally for better read performance
///
/// # Cache Update Strategy
/// Same as `compute_diff`:
/// - Initial/NeedsReload/Unchanged: Update cache immediately
/// - Edits: DON'T update - caller updates after successful broadcast
pub fn compute_diff_shared(
    cache: &SharedCache,
    key: CacheKey,
    new_vdom: Document<Indexed>,
) -> DiffOutcome {
    // Try to get old VDOM (read lock)
    if let Some(old_entry) = cache.get(&key) {
        let outcome = diff_vdom(&old_entry.doc, &new_vdom);

        match &outcome {
            DiffOutcome::NeedsReload { .. } | DiffOutcome::Unchanged => {
                // Safe to update cache - browser will reload or content is same
                cache.insert(key, cache_entry_from_indexed(&new_vdom));
            }
            DiffOutcome::Edits(..) => {
                // DON'T update cache - caller updates after successful broadcast
            }
            DiffOutcome::Initial => unreachable!("old_vdom exists"),
        }

        outcome
    } else {
        // Initial - insert into cache (write lock)
        cache.insert(key, cache_entry_from_indexed(&new_vdom));
        DiffOutcome::Initial
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::family::{IndexedDocument, Raw, TolaSite};
    use tola_vdom::Element;
    use tola_vdom::transform::Transform;

    /// Helper to create an Indexed document for testing
    fn make_indexed_doc(tag: &str) -> IndexedDocument {
        let root: Element<Raw> = Element::new(tag);
        let raw_doc = Document::new(root);
        TolaSite::indexer().transform(raw_doc)
    }

    #[test]
    fn test_diff_outcome_variants() {
        let _ = DiffOutcome::Initial;
        let _ = DiffOutcome::Unchanged;
        let _ = DiffOutcome::NeedsReload {
            reason: "test".to_string(),
        };
    }

    #[test]
    fn test_diff_vdom_unchanged() {
        let doc1 = make_indexed_doc("html");
        let doc2 = make_indexed_doc("html");
        let old = tola_vdom::snapshot::project(&doc1);
        let outcome = diff_vdom(&old, &doc2);
        assert!(matches!(outcome, DiffOutcome::Unchanged));
    }

    #[test]
    fn test_empty_cache_returns_initial() {
        let mut cache: Cache = Cache::default();
        let doc = make_indexed_doc("html");
        let key = CacheKey::new("/test");
        let outcome = compute_diff(&mut cache, key, doc);
        assert!(matches!(outcome, DiffOutcome::Initial));
    }
}
