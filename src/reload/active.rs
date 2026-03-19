//! Active Page Tracking
//!
//! Tracks currently active pages for hot-reload prioritization.
//! Supports multiple browser tabs viewing different pages.

use std::sync::LazyLock;

use dashmap::DashMap;

use crate::core::UrlPath;

// =============================================================================
// Active Page Tracker
// =============================================================================

/// Tracks all currently active pages across connected clients.
///
/// Thread-safe. Supports multiple browser tabs viewing different pages.
///
/// This tracker is intentionally fed by browser runtime messages after a page
/// has already loaded. It is therefore useful for subsequent hot-reload
/// prioritization, but it is not the signal that drives the first HTTP
/// request-time compile for a page.
pub struct ActivePageTracker {
    active_urls: DashMap<UrlPath, usize>,
}

impl ActivePageTracker {
    pub fn new() -> Self {
        Self {
            active_urls: DashMap::new(),
        }
    }

    /// Add an active page (called when client reports current URL).
    pub fn add(&self, url_path: impl Into<UrlPath>) {
        let url_path = url_path.into();
        self.active_urls
            .entry(url_path)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }

    /// Remove an active page (called when client disconnects or navigates away).
    pub fn remove(&self, url_path: &UrlPath) {
        if let Some(mut count) = self.active_urls.get_mut(url_path) {
            if *count > 1 {
                *count -= 1;
            } else {
                drop(count);
                self.active_urls.remove(url_path);
            }
        }
    }

    /// Get all active page URLs.
    pub fn get_all(&self) -> Vec<UrlPath> {
        self.active_urls
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Check if a URL is currently active (any client viewing it).
    pub fn is_active(&self, url_path: &str) -> bool {
        self.active_urls
            .iter()
            .any(|entry| entry.key().matches_ignoring_trailing_slash(url_path))
    }

    /// Clear all active pages (called when all clients disconnect).
    pub fn clear(&self) {
        self.active_urls.clear();
    }
}

impl Default for ActivePageTracker {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Global Instance
// =============================================================================

/// Global active page tracker
///
/// Used by WsActor to record clients' current pages,
/// and by CompilerActor to prioritize subsequent hot-reload work.
pub static ACTIVE_PAGE: LazyLock<ActivePageTracker> = LazyLock::new(ActivePageTracker::new);

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_page_tracker() {
        let tracker = ActivePageTracker::new();

        assert!(tracker.get_all().is_empty());

        // Add multiple pages
        tracker.add(UrlPath::from_page("/blog/post-1/"));
        tracker.add(UrlPath::from_page("/blog/post-2/"));
        tracker.add(UrlPath::from_page("/about/"));

        assert_eq!(tracker.get_all().len(), 3);
        assert!(tracker.is_active("/blog/post-1/"));
        assert!(tracker.is_active("/blog/post-1")); // ignores trailing slash
        assert!(tracker.is_active("/blog/post-2/"));
        assert!(tracker.is_active("/about/"));

        // Remove one
        tracker.remove(&UrlPath::from_page("/blog/post-1/"));
        assert!(!tracker.is_active("/blog/post-1/"));
        assert!(tracker.is_active("/blog/post-2/"));

        tracker.clear();
        assert!(tracker.get_all().is_empty());
    }

    #[test]
    fn test_duplicate_routes_require_matching_removals() {
        let tracker = ActivePageTracker::new();
        let route = UrlPath::from_page("/blog/post-1/");

        tracker.add(route.clone());
        tracker.add(route.clone());
        assert!(tracker.is_active(route.as_str()));
        assert_eq!(tracker.get_all(), vec![route.clone()]);

        tracker.remove(&route);
        assert!(tracker.is_active(route.as_str()));

        tracker.remove(&route);
        assert!(!tracker.is_active(route.as_str()));
    }
}
