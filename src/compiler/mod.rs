//! Compilation for static site generation.

pub mod dependency;
pub mod family;
pub mod page;
pub mod scheduler;

use jwalk::WalkDir;
use std::path::{Path, PathBuf};

use crate::config::SiteConfig;
use crate::core::BuildMode;
use page::PageRoute;

pub use page::drain_warnings;

/// Context for the compilation pipeline
pub struct CompileContext<'a> {
    pub mode: BuildMode,
    pub config: &'a SiteConfig,
    pub route: Option<&'a PageRoute>,
    /// Whether to inject global header content (styles, scripts, elements).
    /// Default: `true`. Set to `false` for pages like 404 that need
    /// self-contained styles to avoid relative path issues.
    pub global_header: bool,
}

impl<'a> CompileContext<'a> {
    pub fn new(mode: BuildMode, config: &'a SiteConfig) -> Self {
        Self {
            mode,
            config,
            route: None,
            global_header: true,
        }
    }

    pub fn with_route(mut self, route: &'a PageRoute) -> Self {
        self.route = Some(route);
        self
    }

    /// Set whether to inject global header content.
    #[allow(dead_code)]
    pub fn with_global_header(mut self, global_header: bool) -> Self {
        self.global_header = global_header;
        self
    }

    /// Get the permalink for StableId seeding.
    pub fn permalink(&self) -> Option<&str> {
        self.route.map(|r| r.permalink.as_str())
    }
}

const IGNORED_FILES: &[&str] = &[".DS_Store"];

/// Collect all files from a directory recursively
pub fn collect_all_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let name = e.file_name().to_str().unwrap_or_default();
            !IGNORED_FILES.contains(&name)
        })
        .map(|e| e.path())
        .collect()
}
