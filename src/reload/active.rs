//! Active Page Tracking
//!
//! Tracks currently active pages for compilation prioritization.
//! Supports multiple browser tabs viewing different pages.

use std::sync::LazyLock;

use dashmap::DashSet;

use crate::core::UrlPath;

// =============================================================================
// Active Page Tracker
// =============================================================================

/// Tracks all currently active pages across connected clients.
///
/// Thread-safe. Supports multiple browser tabs viewing different pages.
pub struct ActivePageTracker {
    active_urls: DashSet<UrlPath>,
}

impl ActivePageTracker {
    pub fn new() -> Self {
        Self {
            active_urls: DashSet::new(),
        }
    }

    /// Add an active page (called when client reports current URL).
    pub fn add(&self, url_path: impl Into<UrlPath>) {
        self.active_urls.insert(url_path.into());
    }

    /// Remove an active page (called when client disconnects or navigates away).
    pub fn remove(&self, url_path: &UrlPath) {
        self.active_urls.remove(url_path);
    }

    /// Get all active page URLs.
    pub fn get_all(&self) -> Vec<UrlPath> {
        self.active_urls.iter().map(|r| r.clone()).collect()
    }

    /// Check if a URL is currently active (any client viewing it).
    pub fn is_active(&self, url_path: &str) -> bool {
        self.active_urls
            .iter()
            .any(|u| u.matches_ignoring_trailing_slash(url_path))
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

/// Global active page tracker.
///
/// Used by WsActor to record clients' current pages,
/// and by CompilerActor to prioritize compilation.
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
}
