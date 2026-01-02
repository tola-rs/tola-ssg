//! Diff Pipeline - VDOM Diffing and Patch Generation
//!
//! Contains both pure and effectful functions for computing VDOM diffs.
//!
//! # Function Types
//!
//! - `diff_vdom()` - Pure function, computes diff without side effects
//! - `compute_diff()` - Effectful, updates cache (used by VdomActor)
//! - `compute_diff_shared()` - Thread-safe version using `SharedCache`

use crate::compiler::family::{Cache, CacheEntry, Indexed, PatchOp, SharedCache};
use tola_vdom::algo::{DiffResult, diff};
use tola_vdom::{CacheKey, Document};

/// Outcome of diff computation
#[derive(Debug)]
pub enum DiffOutcome {
    /// First time seeing this page, no diff possible
    Initial,
    /// No changes detected
    Unchanged,
    /// Patch operations to apply (includes new VDOM for cache update after broadcast).
    /// Contains pure `PatchOp` - caller should render to `Patch` before sending to browser.
    /// Document is boxed to reduce enum size.
    Patches(Vec<PatchOp>, Box<Document<Indexed>>),
    /// Structural change requires full reload
    NeedsReload { reason: String },
}

// =============================================================================
// Pure Function (no side effects)
// =============================================================================

/// Compute diff between old and new VDOM (pure function).
///
/// Returns diff result without modifying any state.
/// Use this when you want to check what would change.
pub fn diff_vdom(old_vdom: &Document<Indexed>, new_vdom: &Document<Indexed>) -> DiffOutcome {
    let diff_result: DiffResult<Indexed> = diff(old_vdom, new_vdom);

    if diff_result.should_reload {
        DiffOutcome::NeedsReload {
            reason: diff_result
                .reload_reason
                .unwrap_or_else(|| "complex change".to_string()),
        }
    } else if diff_result.ops.is_empty() {
        DiffOutcome::Unchanged
    } else {
        DiffOutcome::Patches(diff_result.ops, Box::new(new_vdom.clone()))
    }
}

// =============================================================================
// Effectful Function (modifies cache)
// =============================================================================

/// Compute diff and update cache appropriately.
///
/// # Side Effects
/// - Reads from cache
/// - Writes to cache (for Initial/Reload/Unchanged cases)
///
/// # Cache Update Strategy
/// - Initial: Insert new VDOM (browser will reload anyway)
/// - NeedsReload: Insert new VDOM (browser will reload anyway)
/// - Unchanged: Insert new VDOM (safe, content identical)
/// - Patches: DON'T update cache here - caller updates after successful broadcast
///
/// # Note
/// Caller must create `CacheKey` explicitly to ensure URL normalization.
#[allow(dead_code)]
pub fn compute_diff(cache: &mut Cache, key: CacheKey, new_vdom: Document<Indexed>) -> DiffOutcome {
    if let Some(old_entry) = cache.get(&key) {
        let outcome = diff_vdom(&old_entry.doc, &new_vdom);

        match &outcome {
            DiffOutcome::NeedsReload { .. } | DiffOutcome::Unchanged => {
                // Safe to update cache - browser will reload or content is same
                cache.insert(key, CacheEntry::with_default_version(new_vdom));
            }
            DiffOutcome::Patches(..) => {
                // DON'T update cache - caller updates after successful broadcast
                // This keeps cache in sync with what browser actually displays
            }
            DiffOutcome::Initial => unreachable!("old_vdom exists"),
        }

        outcome
    } else {
        // Initial - insert into cache
        cache.insert(key, CacheEntry::with_default_version(new_vdom));
        DiffOutcome::Initial
    }
}

/// Thread-safe version of `compute_diff` using `SharedCache`.
///
/// This version is suitable for concurrent access from multiple threads.
/// Uses `RwLock` internally for better read performance.
///
/// # Cache Update Strategy
/// Same as `compute_diff`:
/// - Initial/NeedsReload/Unchanged: Update cache immediately
/// - Patches: DON'T update - caller updates after successful broadcast
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
                cache.insert(key, CacheEntry::with_default_version(new_vdom));
            }
            DiffOutcome::Patches(..) => {
                // DON'T update cache - caller updates after successful broadcast
            }
            DiffOutcome::Initial => unreachable!("old_vdom exists"),
        }

        outcome
    } else {
        // Initial - insert into cache (write lock)
        cache.insert(key, CacheEntry::with_default_version(new_vdom));
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
        let outcome = diff_vdom(&doc1, &doc2);
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
