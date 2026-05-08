use std::path::Path;

use super::{StoredPage, StoredPageMap};
use crate::compiler::page::ScannedHeading;
use crate::core::UrlPath;

/// Controls whether stale backlink graph entries are cleared when permalink changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleLinkPolicy {
    /// Keep existing link graph entries for old permalink.
    Keep,
    /// Clear link graph entries for old permalink.
    Clear,
}

pub struct PageState<'a> {
    pages: &'a StoredPageMap,
}

impl<'a> PageState<'a> {
    pub fn new(pages: &'a StoredPageMap) -> Self {
        Self { pages }
    }

    pub fn sync_source_permalink(
        &self,
        source: &Path,
        new_permalink: UrlPath,
        stale_link_policy: StaleLinkPolicy,
    ) {
        let old_permalink = self.pages.sync_source_permalink(source, new_permalink);
        if matches!(stale_link_policy, StaleLinkPolicy::Clear)
            && let Some(old_permalink) = old_permalink
        {
            self.clear_links(&old_permalink);
        }
    }

    pub fn build_current_context(&self, url: &UrlPath, path: Option<&str>) -> serde_json::Value {
        use crate::package::TolaPackage;

        let parent = url.parent().map(|p| p.as_str().to_string());
        let filename = path.and_then(|s| {
            Path::new(s)
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
        });

        let links_to = self.pages_for_urls(&self.links_to(url));
        let linked_by = self.pages_for_urls(&self.linked_by(url));
        let headings = self.pages.get_headings(url);

        serde_json::json!({
            TolaPackage::Current.input_key(): {
                "current-permalink": url.as_str(),
                "parent-permalink": parent,
                "path": path,
                "filename": filename,
                "links_to": links_to,
                "linked_by": linked_by,
                "headings": headings,
            }
        })
    }

    fn pages_for_urls(&self, urls: &[UrlPath]) -> Vec<StoredPage> {
        let pages = self.pages.get_pages_with_drafts();
        urls.iter()
            .filter_map(|url| pages.iter().find(|page| page.permalink == *url).cloned())
            .collect()
    }

    pub fn insert_headings(&self, permalink: UrlPath, headings: Vec<ScannedHeading>) {
        self.pages.insert_headings(permalink, headings);
    }

    pub fn record_links(&self, from: &UrlPath, targets: Vec<UrlPath>) {
        self.pages.links().record(from, targets);
    }

    pub fn clear_links(&self, page: &UrlPath) {
        self.record_links(page, vec![]);
    }

    pub fn links_to(&self, page: &UrlPath) -> Vec<UrlPath> {
        self.pages.links().links_to(page)
    }

    pub fn linked_by(&self, page: &UrlPath) -> Vec<UrlPath> {
        self.pages.links().linked_by(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::UrlPath;
    use crate::package::TolaPackage;
    use crate::page::{PageMeta, StaleLinkPolicy};
    use std::path::PathBuf;

    #[test]
    fn sync_source_permalink_clears_stale_links_from_owned_graph() {
        let pages = StoredPageMap::new();
        let state = PageState::new(&pages);

        let source = PathBuf::from("/site/content/post.md");
        let old = UrlPath::from_page("/old/");
        let new = UrlPath::from_page("/new/");
        let target = UrlPath::from_page("/target/");

        pages.insert_source_mapping(source.clone(), old.clone());
        pages.insert_page(
            old.clone(),
            PageMeta {
                title: Some("Old".to_string()),
                ..Default::default()
            },
        );
        state.record_links(&old, vec![target.clone()]);

        state.sync_source_permalink(&source, new.clone(), StaleLinkPolicy::Clear);

        assert_eq!(pages.get_permalink_by_source(&source), Some(new));
        assert!(
            pages
                .get_pages_with_drafts()
                .iter()
                .all(|p| p.permalink != old)
        );
        assert!(state.links_to(&old).is_empty());
        assert!(state.linked_by(&target).is_empty());
    }

    #[test]
    fn current_context_reads_links_from_owned_graph() {
        let pages = StoredPageMap::new();
        let state = PageState::new(&pages);

        let current = UrlPath::from_page("/current/");
        let target = UrlPath::from_page("/target/");
        let source = UrlPath::from_page("/source/");

        pages.insert_page(
            current.clone(),
            PageMeta {
                title: Some("Current".to_string()),
                ..Default::default()
            },
        );
        pages.insert_page(
            target.clone(),
            PageMeta {
                title: Some("Target".to_string()),
                ..Default::default()
            },
        );
        pages.insert_page(
            source.clone(),
            PageMeta {
                title: Some("Source".to_string()),
                ..Default::default()
            },
        );
        state.record_links(&current, vec![target.clone()]);
        state.record_links(&source, vec![current.clone()]);

        let context = state.build_current_context(&current, Some("current.md"));
        let payload = &context[TolaPackage::Current.input_key()];

        assert_eq!(payload["current-permalink"], "/current/");
        assert_eq!(payload["filename"], "current.md");
        assert_eq!(payload["links_to"][0]["permalink"], "/target/");
        assert_eq!(payload["linked_by"][0]["permalink"], "/source/");
    }
}
