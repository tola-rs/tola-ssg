//! The input evidence one attempt observed.
//!
//! Producers record the logical and physical boundaries they read here. The vocabulary is
//! independent of the caches that decide freshness, so a realized revision can hold the
//! observation of the attempt that produced it.

/// Filesystem membership range observed by a build input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputPathScope {
    Children,
    Exact,
    Recursive,
}

/// One producer's logical or physical filesystem input boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObservedInputPath {
    path: std::path::PathBuf,
    input: InputKind,
    scope: InputPathScope,
    required: bool,
    asset_selection: Option<(
        std::path::PathBuf,
        std::sync::Arc<tola_input::FileSelection>,
    )>,
}

impl ObservedInputPath {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn input(&self) -> InputKind {
        self.input
    }
    pub fn scope(&self) -> InputPathScope {
        self.scope
    }
    pub fn required(&self) -> bool {
        self.required
    }
    /// The owner-relative tree source and its selection, when this is an asset tree.
    pub fn asset_selection(&self) -> Option<(&std::path::Path, &tola_input::FileSelection)> {
        self.asset_selection
            .as_ref()
            .map(|(source, selection)| (source.as_path(), selection.as_ref()))
    }
}

/// Public observation view, independent of producer cache implementations.
#[derive(Debug, Clone, Default)]
pub struct InputObservation {
    pub(crate) physical_reads: Vec<std::path::PathBuf>,
    pub(crate) package_checks: Vec<tola_typst::PackageCheck>,
    pub(crate) paths: Vec<ObservedInputPath>,
}

impl InputObservation {
    pub fn physical_read_paths(&self) -> &[std::path::PathBuf] {
        &self.physical_reads
    }
    pub fn package_checks(&self) -> &[tola_typst::PackageCheck] {
        &self.package_checks
    }
    pub fn paths(&self) -> &[ObservedInputPath] {
        &self.paths
    }

    pub(crate) fn normalize_paths(&mut self) {
        self.paths.sort();
        self.paths.dedup();
    }

    /// Record paths the attempt must stat exactly, without watching their children.
    pub(crate) fn exact_paths(
        &mut self,
        input: InputKind,
        required: bool,
        paths: impl IntoIterator<Item = std::path::PathBuf>,
    ) {
        self.paths
            .extend(paths.into_iter().map(|path| ObservedInputPath {
                path,
                input,
                scope: InputPathScope::Exact,
                required,
                asset_selection: None,
            }));
    }

    /// Record one logical boundary and its resolved physical identity.
    pub(crate) fn path_pair(
        &mut self,
        input: InputKind,
        scope: InputPathScope,
        required: bool,
        logical_path: &std::path::Path,
        physical_path: &std::path::Path,
    ) {
        self.paths
            .extend([logical_path, physical_path].map(|path| ObservedInputPath {
                path: path.to_path_buf(),
                input,
                scope,
                required,
                asset_selection: None,
            }));
    }

    pub(crate) fn selected_tree_paths(
        &mut self,
        input: InputKind,
        required: bool,
        logical_path: &std::path::Path,
        physical_path: &std::path::Path,
        source_prefix: &std::path::Path,
        selection: &tola_input::FileSelection,
    ) {
        let selection = std::sync::Arc::new(selection.clone());
        self.paths
            .extend([logical_path, physical_path].map(|path| ObservedInputPath {
                path: path.to_path_buf(),
                input,
                scope: InputPathScope::Recursive,
                required,
                asset_selection: Some((source_prefix.to_path_buf(), selection.clone())),
            }));
    }

    pub(crate) fn filesystem_sources(
        &mut self,
        input: InputKind,
        evidence: &crate::filesystem::FilesystemWatchEvidence,
        required: bool,
    ) {
        for boundary in evidence.boundaries() {
            let scope = match boundary.kind() {
                crate::filesystem::FilesystemSourceKind::File => InputPathScope::Exact,
                crate::filesystem::FilesystemSourceKind::Tree => InputPathScope::Recursive,
            };
            self.path_pair(
                input,
                scope,
                required,
                boundary.logical_path(),
                boundary.physical_path(),
            );
        }
    }
}

/// One source of evidence that can invalidate a complete site candidate.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum InputKind {
    TypstPhysicalReads,
    ModuleManifests,
    TypstPackageSelection,
    ContentInventory,
    ConfiguredAssets,
    Icons,
    FontInventory,
    BeforeBuildHookOutputs,
}

impl InputKind {
    /// Author-facing label naming the configuration key that owns these inputs.
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::TypstPhysicalReads => "files read by the Bundle",
            Self::ModuleManifests => "module manifests",
            Self::TypstPackageSelection => "Typst packages",
            Self::ContentInventory => "`build.content-dir` files",
            Self::ConfiguredAssets => "`assets` files",
            Self::Icons => "`icons` sources",
            Self::FontInventory => "`fonts` files",
            Self::BeforeBuildHookOutputs => "`hooks.before-build` generates",
        }
    }
}
