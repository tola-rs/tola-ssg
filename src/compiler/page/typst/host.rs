//! Typst compilation support for tola-ssg.
//!
//! Tola's host-side Typst wiring.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst_batch::prelude::*;

use crate::{config::SiteConfig, package};

/// Nested asset mapping: (output_name, source_path)
///
/// Example: `("images", "assets/images")` maps `/images/photo.webp` to `assets/images/photo.webp`
pub type NestedMapping = (String, PathBuf);

/// Tola's virtual file system for:
/// - `@tola/site:0.0.0` and `@tola/current:0.0.0` packages
/// - Nested asset path mapping (e.g., `/images/xxx` -> `assets/images/xxx`)
pub struct TolaVirtualFS {
    root: PathBuf,
    nested_mappings: Vec<NestedMapping>,
}

impl TolaVirtualFS {
    /// Create a new VFS with nested asset mappings.
    pub fn new(root: PathBuf, nested_mappings: Vec<NestedMapping>) -> Self {
        Self {
            root,
            nested_mappings,
        }
    }
}

impl typst_batch::VirtualFileSystem for TolaVirtualFS {
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        let path_str = path.to_str()?;
        let trimmed = path_str.trim_start_matches('/');
        let first_segment = trimmed.split('/').next()?;

        for (output_name, source) in &self.nested_mappings {
            if output_name == first_segment {
                let rest = trimmed.strip_prefix(first_segment).unwrap_or("");
                let rest = rest.trim_start_matches('/');
                let real_path = if rest.is_empty() {
                    source.clone()
                } else {
                    source.join(rest)
                };
                return std::fs::read(self.root.join(real_path)).ok();
            }
        }
        None
    }

    fn read_package(&self, pkg: &PackageId, path: &str) -> Option<Vec<u8>> {
        package::read_package(pkg, path)
    }
}

/// Host-provided Typst capabilities for one site/configuration.
#[derive(Clone)]
pub struct TypstHost {
    files: FileResolver,
    file_cache: Arc<SharedFileCache>,
    fonts: Arc<FontStore>,
}

impl TypstHost {
    /// Create a host for Typst compilation.
    pub fn new(font_dirs: &[&Path], root: PathBuf, nested_mappings: Vec<NestedMapping>) -> Self {
        Self::new_with_packages(font_dirs, root, nested_mappings, None, None)
    }

    fn new_with_packages(
        font_dirs: &[&Path],
        root: PathBuf,
        nested_mappings: Vec<NestedMapping>,
        package_path: Option<&Path>,
        package_cache_path: Option<&Path>,
    ) -> Self {
        let fonts = typst_batch::warmup(font_dirs);
        Self {
            files: file_resolver(root, nested_mappings, package_path, package_cache_path),
            file_cache: Arc::new(SharedFileCache::new()),
            fonts,
        }
    }

    /// Create a host from a site config.
    pub fn for_config(config: &SiteConfig) -> Self {
        let font_dirs = font_dirs(config);
        Self::new_with_packages(
            &font_dirs,
            config.get_root().to_path_buf(),
            build_nested_mappings(&config.build.assets.nested),
            config.package_path(),
            config.package_cache_path(),
        )
    }

    /// Create a compile builder using this host.
    pub fn compiler<'a>(&self, root: &'a Path) -> Compiler<'a> {
        Compiler::new(root)
            .with_files(self.files.clone())
            .with_file_cache(Arc::clone(&self.file_cache))
            .with_font_store(Arc::clone(&self.fonts))
    }

    /// Create a scan builder using this host.
    pub fn scanner<'a>(&self, root: &'a Path) -> Scanner<'a> {
        Scanner::new(root).with_files(self.files.clone())
    }

    /// Create a batch compile builder using this host.
    pub fn batcher<'a>(&self, root: &'a Path) -> Batcher<'a> {
        self.compiler(root).into_batch()
    }

    /// Create a batch scan builder using this host.
    pub fn batch_scanner<'a>(&self, root: &'a Path) -> typst_batch::process::BatchScanner<'a> {
        Batcher::for_scan(root).with_files(self.files.clone())
    }

    /// Clear this host's file cache.
    pub fn clear_cache(&self) {
        self.file_cache.clear();
    }

    /// Check whether this host resolves a virtual path.
    #[cfg(test)]
    pub fn is_virtual_path(&self, path: &Path) -> bool {
        self.files.is_virtual_path(path)
    }
}

fn file_resolver(
    root: PathBuf,
    nested_mappings: Vec<NestedMapping>,
    package_path: Option<&Path>,
    package_cache_path: Option<&Path>,
) -> FileResolver {
    let mut files = FileResolver::new().with_virtual_fs(TolaVirtualFS::new(root, nested_mappings));
    if let Some(path) = package_path {
        files = files.with_package_path(path);
    }
    if let Some(path) = package_cache_path {
        files = files.with_package_cache_path(path);
    }
    files
}

/// Collect font directories from site config.
fn font_dirs(config: &SiteConfig) -> Vec<&Path> {
    let mut dirs: Vec<&Path> = vec![config.build.content.as_path()];
    dirs.extend(config.build.assets.nested_sources());
    dirs.extend(config.build.deps.iter().map(|p| p.as_path()));
    dirs
}

/// Build nested mappings from assets config.
fn build_nested_mappings(
    nested: &[crate::config::section::build::assets::NestedEntry],
) -> Vec<NestedMapping> {
    nested
        .iter()
        .map(|entry| {
            (
                entry.output_name().to_string(),
                entry.source().to_path_buf(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn host_without_mappings_does_not_panic() {
        let dir = TempDir::new().unwrap();
        TypstHost::new(&[dir.path()], dir.path().to_path_buf(), Vec::new());
    }

    #[test]
    fn host_with_mappings_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let mappings = vec![("images".to_string(), PathBuf::from("assets/images"))];
        TypstHost::new(&[dir.path()], dir.path().to_path_buf(), mappings);
    }
}
