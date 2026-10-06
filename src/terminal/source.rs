//! Whole-file source text for diagnostics the terminal renders.
//!
//! `codespan-reporting` shows a snippet by asking for the lines a label covers, exactly as the
//! Typst CLI does through `DiagnosticWorld`. Tola names files the way the site author wrote them,
//! so this resolves those names back to bytes and keeps each file's text for the whole rendering
//! session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tola_typst::FileResolver;

/// Site root and package locations one command's diagnostics resolve against.
pub(crate) struct SourceFiles {
    root: PathBuf,
    resolver: FileResolver,
    files: Mutex<HashMap<String, Option<Arc<str>>>>,
}

impl SourceFiles {
    pub(crate) fn new(root: PathBuf, packages: tola_typst::PackageLocations) -> Self {
        Self {
            root,
            resolver: FileResolver::from_package_locations(
                packages,
                tola_typst::PackageFetchPolicy::LocalOnly,
            ),
            files: Mutex::new(HashMap::new()),
        }
    }

    /// The whole text of one diagnostic path, read at most once per session.
    ///
    /// A path the site does not hold, or one that is not UTF-8, yields `None`.
    pub(crate) fn read(&self, path: &str) -> Option<Arc<str>> {
        if let Ok(files) = self.files.lock()
            && let Some(cached) = files.get(path)
        {
            return cached.clone();
        }
        let text = self
            .resolver
            .read_reported(&self.root, Path::new(path))
            .ok()
            .map(Arc::from);
        if let Ok(mut files) = self.files.lock() {
            files.insert(path.to_owned(), text.clone());
        }
        text
    }
}
