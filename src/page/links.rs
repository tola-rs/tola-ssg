//! Page link graph for links-to / linked-by tracking.
//!
//! Tracks internal links between pages:
//! - `links_to`: Pages that this page links to (outgoing)
//! - `linked_by`: Pages that link to this page (incoming/backlinks)

use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::LazyLock;

use crate::core::UrlPath;

type UrlSet = FxHashSet<UrlPath>;
type UrlSetMap = FxHashMap<UrlPath, UrlSet>;

/// Global page link graph.
pub static PAGE_LINKS: LazyLock<PageLinkGraph> = LazyLock::new(PageLinkGraph::new);

/// Bidirectional page link graph.
///
/// Tracks which pages link to which other pages.
#[derive(Debug, Default)]
pub struct PageLinkGraph {
    /// Forward: page → pages it links to (outgoing links)
    links_to: RwLock<UrlSetMap>,
    /// Reverse: page → pages that link to it (backlinks)
    linked_by: RwLock<UrlSetMap>,
}

impl PageLinkGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all link data.
    pub fn clear(&self) {
        self.links_to.write().clear();
        self.linked_by.write().clear();
    }

    /// Record outgoing links from a page.
    ///
    /// Replaces any existing links for this page.
    pub fn record(&self, from: &UrlPath, targets: Vec<UrlPath>) {
        // Remove old mappings first
        self.remove_page(from);

        if targets.is_empty() {
            return;
        }

        // Build target set (exclude self-links)
        let target_set: UrlSet = targets.into_iter().filter(|t| t != from).collect();

        // Update reverse mapping (linked_by)
        {
            let mut linked_by = self.linked_by.write();
            for target in &target_set {
                linked_by
                    .entry(target.clone())
                    .or_default()
                    .insert(from.clone());
            }
        }

        // Store forward mapping (links_to)
        self.links_to.write().insert(from.clone(), target_set);
    }

    /// Get pages that this page links to (outgoing).
    pub fn links_to(&self, page: &UrlPath) -> Vec<UrlPath> {
        self.links_to
            .read()
            .get(page)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get pages that link to this page (backlinks).
    pub fn linked_by(&self, page: &UrlPath) -> Vec<UrlPath> {
        self.linked_by
            .read()
            .get(page)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove a page and clean up its mappings.
    fn remove_page(&self, page: &UrlPath) {
        // Remove from links_to and clean up linked_by
        if let Some(old_targets) = self.links_to.write().remove(page) {
            let mut linked_by = self.linked_by.write();
            for target in old_targets {
                if let Some(sources) = linked_by.get_mut(&target) {
                    sources.remove(page);
                    if sources.is_empty() {
                        linked_by.remove(&target);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> UrlPath {
        UrlPath::from_page(s)
    }

    #[test]
    fn test_basic_recording() {
        let graph = PageLinkGraph::new();

        let page_a = url("/a/");
        let page_b = url("/b/");
        let page_c = url("/c/");

        // A links to B and C
        graph.record(&page_a, vec![page_b.clone(), page_c.clone()]);

        // Check links_to
        let links = graph.links_to(&page_a);
        assert!(links.contains(&page_b));
        assert!(links.contains(&page_c));

        // Check linked_by
        assert!(graph.linked_by(&page_b).contains(&page_a));
        assert!(graph.linked_by(&page_c).contains(&page_a));
    }

    #[test]
    fn test_self_link_excluded() {
        let graph = PageLinkGraph::new();

        let page_a = url("/a/");
        let page_b = url("/b/");

        // A links to itself and B
        graph.record(&page_a, vec![page_a.clone(), page_b.clone()]);

        // Self-link should be excluded
        let links = graph.links_to(&page_a);
        assert!(!links.contains(&page_a));
        assert!(links.contains(&page_b));
    }

    #[test]
    fn test_update_replaces_old() {
        let graph = PageLinkGraph::new();

        let page_a = url("/a/");
        let page_b = url("/b/");
        let page_c = url("/c/");

        // A links to B
        graph.record(&page_a, vec![page_b.clone()]);
        assert!(graph.linked_by(&page_b).contains(&page_a));

        // Update: A now links to C (not B)
        graph.record(&page_a, vec![page_c.clone()]);

        // B should no longer have A as backlink
        assert!(!graph.linked_by(&page_b).contains(&page_a));
        // C should have A as backlink
        assert!(graph.linked_by(&page_c).contains(&page_a));
    }

    #[test]
    fn test_multiple_sources() {
        let graph = PageLinkGraph::new();

        let page_a = url("/a/");
        let page_b = url("/b/");
        let page_c = url("/c/");

        // Both A and B link to C
        graph.record(&page_a, vec![page_c.clone()]);
        graph.record(&page_b, vec![page_c.clone()]);

        let backlinks = graph.linked_by(&page_c);
        assert_eq!(backlinks.len(), 2);
        assert!(backlinks.contains(&page_a));
        assert!(backlinks.contains(&page_b));
    }

    #[test]
    fn test_clear() {
        let graph = PageLinkGraph::new();

        let page_a = url("/a/");
        let page_b = url("/b/");

        graph.record(&page_a, vec![page_b.clone()]);
        assert!(!graph.links_to(&page_a).is_empty());

        graph.clear();
        assert!(graph.links_to(&page_a).is_empty());
        assert!(graph.linked_by(&page_b).is_empty());
    }
}
