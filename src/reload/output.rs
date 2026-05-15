//! Output asset reload protocol.
//!
//! Files under the configured output directory can be written by hooks or by
//! external tools. The watcher sees both as filesystem events, so this module
//! owns the rules for turning output paths into hot-reload asset messages and
//! for filtering watcher echoes of writes already handled by the compiler.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use crate::config::SiteConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    File(String),
    Missing,
}

impl State {
    fn read(path: &Path) -> Self {
        if path.is_file() {
            Self::File(crate::asset::version::compute_version(path))
        } else {
            Self::Missing
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    states: FxHashMap<PathBuf, State>,
}

impl Snapshot {
    fn read(config: &SiteConfig) -> Self {
        let output_dir = config.paths().output_dir();
        if !output_dir.exists() {
            return Self::default();
        }

        let states = jwalk::WalkDir::new(output_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| {
                let path = crate::utils::path::normalize_path(&entry.path());
                is_reloadable(&path, config).then(|| (path.clone(), State::read(&path)))
            })
            .collect();

        Self { states }
    }

    fn changed_since(&self, config: &SiteConfig) -> Vec<PathBuf> {
        let after = Self::read(config);
        let mut changed: Vec<_> = after
            .states
            .iter()
            .filter_map(|(path, state)| match self.states.get(path) {
                Some(old) if old == state => None,
                _ => Some(path.clone()),
            })
            .chain(
                self.states
                    .keys()
                    .filter(|path| !after.states.contains_key(*path))
                    .cloned(),
            )
            .collect();
        changed.sort();
        changed.dedup();
        changed
    }
}

#[derive(Debug, Default)]
pub(crate) struct Echoes {
    states: FxHashMap<PathBuf, State>,
}

impl Echoes {
    pub(crate) fn record(&mut self, paths: &[PathBuf]) {
        for path in paths {
            let path = crate::utils::path::normalize_path(path);
            self.states.insert(path.clone(), State::read(&path));
        }
    }

    pub(crate) fn filter(&mut self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths
            .into_iter()
            .filter_map(|path| {
                let path = crate::utils::path::normalize_path(&path);
                (!self.is_echo(&path)).then_some(path)
            })
            .collect()
    }

    fn is_echo(&mut self, path: &Path) -> bool {
        let Some(recorded) = self.states.get(path) else {
            return false;
        };

        if State::read(path) == *recorded {
            return true;
        }

        self.states.remove(path);
        false
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Update {
    hrefs: Vec<String>,
    reload_count: usize,
}

impl Update {
    pub(crate) fn is_empty(&self) -> bool {
        self.hrefs.is_empty() && self.reload_count == 0
    }

    pub(crate) fn add_href(&mut self, href: String) {
        if self.hrefs.iter().any(|existing| existing == &href) {
            return;
        }
        self.hrefs.push(href);
    }

    pub(crate) fn require_reload(&mut self, count: usize) {
        self.reload_count = self.reload_count.max(count);
    }

    pub(crate) fn extend(&mut self, other: Self) {
        for href in other.hrefs {
            self.add_href(href);
        }
        self.reload_count = self.reload_count.max(other.reload_count);
    }

    pub(crate) fn hrefs(&self) -> &[String] {
        &self.hrefs
    }

    pub(crate) fn reload_count(&self) -> usize {
        self.reload_count
    }
}

pub(crate) fn snapshot(config: &SiteConfig) -> Snapshot {
    Snapshot::read(config)
}

pub(crate) fn changed(before: &Snapshot, config: &SiteConfig) -> Vec<PathBuf> {
    before.changed_since(config)
}

pub(crate) fn href(path: &Path, config: &SiteConfig) -> Option<String> {
    let output_dir = crate::utils::path::normalize_path(&config.paths().output_dir());
    let path = crate::utils::path::normalize_path(path);
    let relative = path.strip_prefix(output_dir).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }

    Some(config.paths().url_for_site_path(relative))
}

pub(crate) fn versioned_href(path: &Path, config: &SiteConfig) -> Option<String> {
    let href = href(path, config)?;
    Some(crate::asset::version::versioned_url(&href, path))
}

pub(crate) fn is_reloadable(path: &Path, config: &SiteConfig) -> bool {
    if matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("html" | "htm")
    ) {
        return false;
    }

    !is_seo_output(path, config)
}

fn is_seo_output(path: &Path, config: &SiteConfig) -> bool {
    let path = crate::utils::path::normalize_path(path);

    config.site.seo.feed_outputs().iter().any(|feed| {
        path == crate::utils::path::normalize_path(&feed.url.output_path(config.paths()))
    }) || (config.site.seo.sitemap.enable
        && path
            == crate::utils::path::normalize_path(
                &config.site.seo.sitemap.url.output_path(config.paths()),
            ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn reloadable_output_asset_excludes_html_and_seo_outputs() {
        let mut config = SiteConfig::default();
        config.build.output = PathBuf::from("/public");
        config.site.seo.feeds = vec![
            crate::config::FeedConfig {
                format: crate::config::FeedFormat::Rss,
                url: "/feed.xml".into(),
                features: vec![],
            },
            crate::config::FeedConfig {
                format: crate::config::FeedFormat::Atom,
                url: "/atom.xml".into(),
                features: vec![],
            },
            crate::config::FeedConfig {
                format: crate::config::FeedFormat::Json,
                url: "/feed.json".into(),
                features: vec![],
            },
        ];
        config.site.seo.sitemap.enable = true;
        config.site.seo.sitemap.url = "/sitemap.xml".into();

        assert!(is_reloadable(Path::new("/public/assets/app.css"), &config));
        assert!(is_reloadable(Path::new("/public/assets/app.js"), &config));
        assert!(!is_reloadable(
            Path::new("/public/page/index.html"),
            &config
        ));
        assert!(!is_reloadable(Path::new("/public/page/index.htm"), &config));
        assert!(!is_reloadable(Path::new("/public/feed.xml"), &config));
        assert!(!is_reloadable(Path::new("/public/atom.xml"), &config));
        assert!(!is_reloadable(Path::new("/public/feed.json"), &config));
        assert!(!is_reloadable(Path::new("/public/sitemap.xml"), &config));
    }

    #[test]
    fn output_asset_href_uses_prefixed_output_dir() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let file = output.join("docs/blog/styles/site.css");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "body{}").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = output;
        config.build.path_prefix = "docs/blog".into();

        assert_eq!(
            href(&file, &config),
            Some("/docs/blog/styles/site.css".into())
        );
    }

    #[test]
    fn output_asset_href_rejects_paths_outside_prefixed_output_dir() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let file = output.join("styles/site.css");

        let mut config = SiteConfig::default();
        config.build.output = output;
        config.build.path_prefix = "docs/blog".into();

        assert_eq!(href(&file, &config), None);
    }

    #[test]
    fn changed_outputs_detects_content_changes_and_skips_html() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let css = output.join("styles/site.css");
        let js = output.join("scripts/app.js");
        let html = output.join("index.html");
        std::fs::create_dir_all(css.parent().unwrap()).unwrap();
        std::fs::create_dir_all(js.parent().unwrap()).unwrap();
        std::fs::write(&css, "body{color:red}").unwrap();
        std::fs::write(&html, "<html></html>").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = output;

        let before = snapshot(&config);
        std::fs::write(&css, "body{color:blue}").unwrap();
        std::fs::write(&js, "console.log(1)").unwrap();
        std::fs::write(&html, "<html><body>changed</body></html>").unwrap();

        let changed = changed(&before, &config);

        let mut expected = vec![
            crate::utils::path::normalize_path(&css),
            crate::utils::path::normalize_path(&js),
        ];
        expected.sort();
        assert_eq!(changed, expected);
    }

    #[test]
    fn changed_outputs_ignores_same_content_rewrites() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let css = output.join("styles/site.css");
        std::fs::create_dir_all(css.parent().unwrap()).unwrap();
        std::fs::write(&css, "body{}").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = output;

        let before = snapshot(&config);
        std::fs::write(&css, "body{}").unwrap();

        assert!(changed(&before, &config).is_empty());
    }

    #[test]
    fn output_echoes_match_same_content_and_reject_changed_content() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("public/data/site.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"a":1}"#).unwrap();

        let mut echoes = Echoes::default();
        echoes.record(std::slice::from_ref(&file));

        assert!(echoes.filter(vec![file.clone()]).is_empty());

        std::fs::write(&file, r#"{"a":2}"#).unwrap();
        assert_eq!(
            echoes.filter(vec![file.clone()]),
            vec![crate::utils::path::normalize_path(&file)]
        );
    }

    #[test]
    fn update_deduplicates_hrefs_when_merged() {
        let mut left = Update::default();
        left.add_href("/styles/site.css?v=one".into());

        let mut right = Update::default();
        right.add_href("/styles/site.css?v=one".into());
        right.add_href("/scripts/app.js?v=two".into());

        left.extend(right);

        assert_eq!(
            left.hrefs(),
            &[
                "/styles/site.css?v=one".to_string(),
                "/scripts/app.js?v=two".to_string()
            ]
        );
    }
}
