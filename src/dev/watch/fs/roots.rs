use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::RecursiveMode;

use super::RebuildScope;
use super::event::{accepted_root_invalidation_kind, is_structural};
use tola_build::config::ResolvedSiteConfig;
use tola_build::filesystem::{lexical_path_identity, normalize_existing_prefix, path_is_within};

/// Declare every build producer once, so its bit index, iteration order, and count cannot drift
/// between the enum, `PRODUCER_KINDS`, and `PRODUCER_COUNT`.
macro_rules! producers {
    ($($variant:ident),+ $(,)?) => {
        /// Build producer affected by a watched input; one input can invalidate several.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub(crate) enum ProducerKind {
            $($variant),+
        }

        /// Every producer, in `ProducerSet` bit order.
        pub(crate) const PRODUCER_KINDS: &[ProducerKind] = &[$(ProducerKind::$variant),+];

        const PRODUCER_COUNT: u32 = PRODUCER_KINDS.len() as u32;
    };
}

producers! {
    Content,
    TypstReads,
    ConfiguredAssets,
    Fonts,
    Hooks,
    PackageResolution,
    Icons,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct ProducerSet(u16);

impl std::fmt::Debug for ProducerSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_set().entries(self.iter()).finish()
    }
}

impl ProducerSet {
    /// Every producer; `PRODUCER_COUNT` is forced into the set's width here, so a producer that
    /// exceeds it stops the build instead of silently shifting out of range.
    pub(crate) const ALL: Self = Self((1_u16 << PRODUCER_COUNT) - 1);

    pub(crate) const fn one(producer: ProducerKind) -> Self {
        Self(1 << producer as u8)
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn contains(self, producer: ProducerKind) -> bool {
        self.0 & Self::one(producer).0 != 0
    }

    pub(crate) fn insert(&mut self, producers: Self) {
        self.0 |= producers.0;
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = ProducerKind> {
        PRODUCER_KINDS
            .iter()
            .copied()
            .filter(move |producer| self.contains(*producer))
    }
}

impl std::ops::BitOr for ProducerSet {
    type Output = Self;

    fn bitor(mut self, rhs: Self) -> Self::Output {
        self.insert(rhs);
        self
    }
}

/// Dynamic requirements discovered outside static site configuration.
#[derive(Debug, Clone, Default)]
pub(crate) struct WatchRequirements {
    requirements: Vec<WatchRequirement>,
}

impl WatchRequirements {
    fn add(
        &mut self,
        scope: WatchScope,
        existence: ExistencePolicy,
        rebuild_scope: RebuildScope,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.requirements.extend(paths.into_iter().map(|path| {
            let requirement = WatchRequirement::for_scope(
                lexical_path_identity(&path),
                scope,
                ProducerSet::one(producer),
            )
            .with_rebuild_scope(rebuild_scope);
            match existence {
                ExistencePolicy::Required => requirement,
                ExistencePolicy::Optional => requirement.optional(),
            }
        }));
    }

    pub(crate) fn exact(
        &mut self,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.add(
            WatchScope::Exact,
            ExistencePolicy::Required,
            RebuildScope::Paths,
            producer,
            paths,
        );
    }

    pub(crate) fn children(
        &mut self,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.add(
            WatchScope::Children,
            ExistencePolicy::Required,
            RebuildScope::Paths,
            producer,
            paths,
        );
    }

    pub(crate) fn recursive(
        &mut self,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.add(
            WatchScope::Recursive,
            ExistencePolicy::Required,
            RebuildScope::Paths,
            producer,
            paths,
        );
    }

    pub(crate) fn package_checks(
        &mut self,
        checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
    ) {
        self.requirements.extend(checks.into_iter().map(|check| {
            // Availability decides existence per check, so this is not one `add` group.
            let requirement = WatchRequirement::for_scope(
                lexical_path_identity(check.candidate()),
                WatchScope::Exact,
                ProducerSet::one(ProducerKind::PackageResolution),
            )
            .with_rebuild_scope(RebuildScope::FullTypst);
            match check.availability() {
                tola_typst::PackageAvailability::Present => requirement,
                tola_typst::PackageAvailability::Missing
                | tola_typst::PackageAvailability::NotDirectory
                | tola_typst::PackageAvailability::Unreadable => requirement.optional(),
            }
        }));
    }

    pub(crate) fn package_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.add(
            WatchScope::Exact,
            ExistencePolicy::Optional,
            RebuildScope::FullTypst,
            ProducerKind::PackageResolution,
            paths,
        );
    }

    pub(crate) fn optional_exact(
        &mut self,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.add(
            WatchScope::Exact,
            ExistencePolicy::Optional,
            RebuildScope::Paths,
            producer,
            paths,
        );
    }

    pub(crate) fn optional_children(
        &mut self,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.add(
            WatchScope::Children,
            ExistencePolicy::Optional,
            RebuildScope::Paths,
            producer,
            paths,
        );
    }

    pub(crate) fn optional_recursive(
        &mut self,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.add(
            WatchScope::Recursive,
            ExistencePolicy::Optional,
            RebuildScope::Paths,
            producer,
            paths,
        );
    }

    pub(crate) fn typst_recovery(&mut self, path: PathBuf) {
        self.add(
            WatchScope::Recursive,
            ExistencePolicy::Optional,
            RebuildScope::FullTypst,
            ProducerKind::TypstReads,
            [path],
        );
    }

    fn iter(&self) -> impl Iterator<Item = &WatchRequirement> {
        self.requirements.iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoverageUncertainty {
    CoverageGap,
    Unreadable,
    MissingRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoverageCertainty {
    AcceptedEvents,
    Uncertain(CoverageUncertainty),
}

/// Coverage and freshness of a producer's watched inputs.
#[derive(Debug, Clone)]
pub(super) struct WatchCoverage {
    established_epoch: u64,
    certainty: CoverageCertainty,
    covers_current_revision: bool,
}

impl WatchCoverage {
    pub(super) const fn established_epoch(&self) -> u64 {
        self.established_epoch
    }

    pub(super) const fn certainty(&self) -> CoverageCertainty {
        self.certainty
    }

    pub(super) const fn covers_current_revision(&self) -> bool {
        self.covers_current_revision
    }
}

/// A native directory watch shared by logical subscriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectoryWatch {
    path: PathBuf,
    mode: RecursiveMode,
}

impl DirectoryWatch {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn recursive_mode(&self) -> RecursiveMode {
        self.mode
    }

    fn covers(&self, required: &Self) -> bool {
        if self.mode == RecursiveMode::Recursive {
            required.path.starts_with(&self.path)
        } else {
            self == required
        }
    }
}

/// A complete physical coverage change, committed only after its batch succeeds.
#[derive(Debug)]
pub(super) enum DirectoryWatchUpdate {
    Retain,
    Replace {
        directories: Vec<DirectoryWatch>,
        operations: Vec<notify::PathOp>,
    },
}

impl DirectoryWatchUpdate {
    pub(super) fn take_operations(&mut self) -> Vec<notify::PathOp> {
        match self {
            Self::Retain => Vec::new(),
            Self::Replace { operations, .. } => std::mem::take(operations),
        }
    }

    pub(super) fn retains_directories(&self) -> bool {
        matches!(self, Self::Retain)
    }
}

/// Logical subscriptions and the minimum directories needed to observe them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchSelection {
    desired: Vec<WatchRequirement>,
    boundary: EventBoundary,
    directories: Vec<DirectoryWatch>,
    directory_producers: BTreeMap<PathBuf, ProducerSet>,
    uncertainties: BTreeMap<ProducerKind, CoverageUncertainty>,
}

impl WatchSelection {
    pub(super) fn boundary(&self) -> EventBoundary {
        self.boundary.clone()
    }

    pub(super) fn directory_producers(&self, path: &Path) -> ProducerSet {
        self.directory_producers
            .get(path)
            .copied()
            .unwrap_or_default()
    }
}

/// Logical event matching and the directory entries that maintain its coverage.
///
/// Ancestor events drive promotion of missing inputs; unrelated siblings remain
/// outside the logical subscription even when they share a recovery directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct EventBoundary {
    /// Sorted by path, allowing event lookup without scanning unrelated subscriptions.
    desired: Arc<[WatchRequirement]>,
    producers: ProducerSet,
    directories: Arc<[PathBuf]>,
    symlink_entries: Arc<[PathBuf]>,
    /// Declared development hook outputs whose current content may account for file events.
    hook_outputs: Arc<[tola_build::filesystem::FilesystemSourceIdentity]>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ClassifiedPath {
    producers: ProducerSet,
    rebuild_scope: RebuildScope,
    /// Whether current hook-output evidence may account for this path.
    hook_output: bool,
}

impl ClassifiedPath {
    pub(super) const fn new(producers: ProducerSet, rebuild_scope: RebuildScope) -> Self {
        Self {
            producers,
            rebuild_scope,
            hook_output: false,
        }
    }

    pub(super) const fn producers(self) -> ProducerSet {
        self.producers
    }

    pub(super) const fn rebuild_scope(self) -> RebuildScope {
        self.rebuild_scope
    }

    pub(super) const fn is_hook_output(self) -> bool {
        self.hook_output
    }

    pub(super) const fn is_empty(self) -> bool {
        self.producers.is_empty()
    }

    fn merge(&mut self, requirement: &WatchRequirement, hook_output: bool) {
        self.producers.insert(requirement.producers);
        // Output events remain pending until content evidence accounts for the write.
        self.rebuild_scope = if hook_output {
            self.rebuild_scope
                .max(requirement.rebuild_scope.min(RebuildScope::Paths))
        } else {
            self.rebuild_scope.max(requirement.rebuild_scope)
        };
        self.hook_output |= hook_output;
    }
}

impl EventBoundary {
    pub(super) fn hook_outputs(&self) -> Arc<[tola_build::filesystem::FilesystemSourceIdentity]> {
        Arc::clone(&self.hook_outputs)
    }

    pub(super) fn classify(&self, path: &Path) -> ClassifiedPath {
        self.classify_change(path, notify::EventKind::Any)
    }

    /// Classify one path of an event whose kind is known.
    ///
    /// A path at or above a subscription's root keeps that subscription: it reports the tree that
    /// holds the input. So does a structural event for a path inside the scope, because a removed
    /// or renamed directory has no file name and its subtree can hide changes no event
    /// reports. A plain content change inside the scope reaches only the subscriptions whose
    /// admission test accepts the path, so a name no producer reads stops waking the session
    /// while every read path keeps its own subscription.
    pub(super) fn classify_change(&self, path: &Path, kind: notify::EventKind) -> ClassifiedPath {
        let structural = is_structural(kind);
        let path = lexical_path_identity(path);
        let hook_output = self.is_declared_hook_output(&path);
        self.intersecting_requirements(&path).fold(
            ClassifiedPath::new(ProducerSet::default(), RebuildScope::Paths),
            |mut classified, requirement| {
                if path_is_within(&requirement.path, &path)
                    || (requirement.matches(&path)
                        && (structural || requirement.admits_change(&path)))
                {
                    classified.merge(requirement, hook_output);
                }
                classified
            },
        )
    }

    /// Whether `path` falls within a declared development hook output.
    pub(super) fn is_declared_hook_output(&self, path: &Path) -> bool {
        self.hook_outputs.iter().any(|output| {
            path_is_within(path, output.logical_path())
                || path_is_within(path, output.physical_path())
        })
    }

    pub(super) fn producers(&self) -> ProducerSet {
        self.producers
    }

    /// Only ancestors and descendants can intersect an event. `desired` is sorted by native
    /// path components: all descendants form one contiguous range, including the root itself.
    /// Ancestors use exact ranges so sibling subscriptions are never visited. Scope and name
    /// admission remain the caller's responsibility.
    fn intersecting_requirements<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = &'a WatchRequirement> + 'a {
        let descendants = self
            .desired
            .partition_point(|item| item.path.as_path() < path);
        let ancestors = path.ancestors().skip(1).flat_map(|ancestor| {
            let start = self
                .desired
                .partition_point(|item| item.path.as_path() < ancestor);
            self.desired[start..]
                .iter()
                .take_while(move |item| item.path == ancestor)
        });
        ancestors.chain(
            self.desired[descendants..]
                .iter()
                .take_while(move |item| item.path.starts_with(path)),
        )
    }

    /// Whether `path`, whose name resembles an editor artifact, is nevertheless an input.
    ///
    /// A name alone cannot tell an artifact from an input, so the subscriptions reaching `path`
    /// decide: one that selects members by name reads only the names that rule matches, while one
    /// that names the file itself or admits every member of its scope keeps it. A path at or above
    /// a subscription's root reports the tree that holds the input, so it stays reachable.
    pub(super) fn admits_editor_artifact(&self, path: &Path) -> bool {
        let path = lexical_path_identity(path);
        self.intersecting_requirements(&path).any(|requirement| {
            path_is_within(&requirement.path, &path)
                || (requirement.matches(&path) && !requirement.selects_members_by_name())
        })
    }

    /// A replaced directory invalidates physical coverage even after its last logical
    /// subscription left; ordinary child edits do not.
    ///
    /// An event naming only declared hook outputs, which replaces those files and not the tree
    /// their subscriptions cover, leaves coverage intact: the hook writes them every run, and
    /// invalidating the tree would supersede every round's candidate. A directory-level
    /// replacement is not such an event, however its paths are named, because it really does
    /// move the tree that holds them.
    pub(super) fn invalidated_by(&self, event: &notify::Event) -> ProducerSet {
        if !accepted_root_invalidation_kind(event.kind) {
            return ProducerSet::default();
        }
        if file_level_replacement(event.kind)
            && !event.paths.is_empty()
            && event
                .paths
                .iter()
                .all(|path| self.is_declared_hook_output(&lexical_path_identity(path)))
        {
            return ProducerSet::default();
        }
        if event.paths.iter().any(|path| {
            let path = lexical_path_identity(path);
            self.directories
                .iter()
                .any(|directory| path_is_within(directory, &path))
                || self
                    .symlink_entries
                    .iter()
                    .any(|entry| path_is_within(entry, &path))
        }) {
            ProducerSet::ALL
        } else {
            ProducerSet::default()
        }
    }
}

/// Whether an event kind replaces one file rather than a directory tree.
///
/// The create and remove kinds hold the file-or-directory distinction the backend proved;
/// every other accepted kind is treated as tree-level, matching `is_structural`.
fn file_level_replacement(kind: notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Create(notify::event::CreateKind::File)
            | notify::EventKind::Remove(notify::event::RemoveKind::File)
    )
}

/// Current logical subscriptions and stable native directory coverage.
pub(super) struct RootSet {
    selection: WatchSelection,
    installed: Vec<DirectoryWatch>,
    coverage: BTreeMap<ProducerKind, WatchCoverage>,
}

impl RootSet {
    fn with_hook_outputs(
        desired: Vec<WatchRequirement>,
        hook_outputs: Arc<[tola_build::filesystem::FilesystemSourceIdentity]>,
    ) -> Self {
        Self {
            selection: selection_for_observed(desired, &mut BTreeMap::new(), hook_outputs),
            installed: Vec::new(),
            coverage: BTreeMap::new(),
        }
    }

    pub(super) fn from_config(config: &ResolvedSiteConfig, dynamic: &WatchRequirements) -> Self {
        Self::with_hook_outputs(
            collect_roots(config, dynamic),
            declared_hook_outputs(config).into(),
        )
    }

    pub(super) fn initial_selection(&self) -> WatchSelection {
        self.selection.clone()
    }

    /// Observe published and attempted configurations together during recovery.
    pub(super) fn configs_selection<'a>(
        &self,
        configs: impl IntoIterator<Item = &'a ResolvedSiteConfig>,
        dynamic: &WatchRequirements,
    ) -> (WatchSelection, bool) {
        let configs: Vec<&ResolvedSiteConfig> = configs.into_iter().collect();
        let mut observed = BTreeMap::new();
        let outputs: Arc<[_]> = declared_hook_outputs_for(&configs).into();
        let configured_roots =
            requirements_for_configs(configs.iter().copied(), &WatchRequirements::default());
        let mut candidate = configured_roots.clone();
        candidate.extend(dynamic.iter().cloned());
        let selection = selection_for_observed(candidate, &mut observed, Arc::clone(&outputs));
        let configured = selection_for_observed(configured_roots, &mut observed, outputs);
        (selection, self.extends_observation(&configured))
    }

    pub(super) fn staged_selection(
        &self,
        config: &ResolvedSiteConfig,
        dynamic: &WatchRequirements,
    ) -> (WatchSelection, WatchSelection, bool) {
        let configured_roots = collect_roots(config, &WatchRequirements::default());
        let mut candidate = configured_roots.clone();
        candidate.extend(dynamic.iter().cloned());
        let mut desired = self.selection.desired.clone();
        desired.extend(candidate.iter().cloned());
        // The cache is scoped to this stage: never reuse resolved symlinks next round.
        let mut observed = BTreeMap::new();
        let hook_outputs: Arc<[_]> = declared_hook_outputs(config).into();
        let observing = selection_for_observed(desired, &mut observed, Arc::clone(&hook_outputs));
        let subscriptions =
            selection_for_observed(candidate, &mut observed, Arc::clone(&hook_outputs));
        let configured = selection_for_observed(configured_roots, &mut observed, hook_outputs);
        // Only declared coverage retains refresh: discovered reads are reverified at handoff.
        (
            observing,
            subscriptions,
            self.extends_observation(&configured),
        )
    }

    pub(super) fn current_selection(&self) -> WatchSelection {
        selection_for_observed(
            self.selection.desired.clone(),
            &mut BTreeMap::new(),
            self.selection.boundary.hook_outputs(),
        )
    }

    pub(super) fn boundary(&self) -> EventBoundary {
        let mut boundary = self.selection.boundary();
        let mut directories = boundary.directories.to_vec();
        directories.extend(
            self.installed
                .iter()
                .map(|directory| directory.path.clone()),
        );
        directories.sort();
        directories.dedup();
        boundary.directories = directories.into();
        boundary
    }

    /// Whether a new input range has not yet been observed; existence diagnostics and rebuild
    /// policy do not enlarge that range.
    pub(super) fn extends_observation(&self, selection: &WatchSelection) -> bool {
        !selection.directories.iter().all(|required| {
            self.installed
                .iter()
                .any(|installed| installed.covers(required))
        }) || selection.boundary.producers().iter().any(|producer| {
            !observation_contains(&self.selection.boundary, &selection.boundary, producer)
        })
    }

    pub(super) fn has_installed_directories(&self) -> bool {
        !self.installed.is_empty()
    }

    /// Logical changes retain native watches; extension or repair installs the minimum
    /// directory set and removes watches no longer needed.
    pub(super) fn directory_update(
        &self,
        selection: &WatchSelection,
        refresh: bool,
    ) -> DirectoryWatchUpdate {
        if !refresh
            && selection.directories.iter().all(|required| {
                self.installed
                    .iter()
                    .any(|installed| installed.covers(required))
            })
        {
            return DirectoryWatchUpdate::Retain;
        }
        let directories = selection.directories.clone();
        let mut operations = Vec::new();
        for directory in &directories {
            if refresh || !self.installed.contains(directory) {
                operations.push(notify::PathOp::Watch(
                    directory.path().to_path_buf(),
                    notify::WatchPathConfig::new(directory.recursive_mode()),
                ));
            }
        }
        for installed in &self.installed {
            if !directories
                .iter()
                .any(|directory| directory.path == installed.path)
            {
                operations.push(notify::PathOp::Unwatch(installed.path.clone()));
            }
        }
        DirectoryWatchUpdate::Replace {
            directories,
            operations,
        }
    }

    /// Commit a fully successful native update, never a failed or partial batch.
    pub(super) fn commit(
        &mut self,
        selection: WatchSelection,
        update: DirectoryWatchUpdate,
        established_epoch: u64,
    ) {
        let physical_changed = match update {
            DirectoryWatchUpdate::Retain => false,
            DirectoryWatchUpdate::Replace { directories, .. } => {
                self.installed = directories;
                true
            }
        };
        self.replace_logical(selection, established_epoch, physical_changed);
    }

    /// Replacement contracts logical subscriptions without touching the backend; also safe
    /// before an extension, where uncovered producers stay uncertain.
    pub(super) fn commit_logical(&mut self, selection: WatchSelection, established_epoch: u64) {
        self.replace_logical(selection, established_epoch, false);
    }

    fn replace_logical(
        &mut self,
        selection: WatchSelection,
        established_epoch: u64,
        physical_changed: bool,
    ) {
        let mut coverage = BTreeMap::new();
        for producer in selection.boundary.producers().iter() {
            let missing_coverage = selection.directories.iter().any(|required| {
                selection
                    .directory_producers(required.path())
                    .contains(producer)
                    && !self
                        .installed
                        .iter()
                        .any(|installed| installed.covers(required))
            });
            let certainty = if missing_coverage {
                CoverageCertainty::Uncertain(CoverageUncertainty::CoverageGap)
            } else {
                selection
                    .uncertainties
                    .get(&producer)
                    .copied()
                    .map(CoverageCertainty::Uncertain)
                    .unwrap_or(CoverageCertainty::AcceptedEvents)
            };
            let previous = self.coverage.get(&producer);
            let continuous = !physical_changed
                && observation_contains(&self.selection.boundary, &selection.boundary, producer)
                && previous.is_some_and(|previous| previous.certainty == certainty);
            let observation = if continuous {
                previous
                    .expect("continuous coverage has a previous observation")
                    .clone()
            } else {
                WatchCoverage {
                    established_epoch,
                    certainty,
                    covers_current_revision: false,
                }
            };
            coverage.insert(producer, observation);
        }
        self.selection = selection;
        self.coverage = coverage;
    }

    pub(super) fn classify(&self, path: &Path) -> ClassifiedPath {
        self.selection.boundary.classify(path)
    }

    pub(super) fn coverage(&self, producer: ProducerKind) -> Option<&WatchCoverage> {
        self.coverage.get(&producer)
    }

    pub(super) fn confirm_current_observation(&mut self) {
        for coverage in self.coverage.values_mut() {
            if coverage.certainty == CoverageCertainty::AcceptedEvents {
                coverage.covers_current_revision = true;
            }
        }
    }
}

fn observation_contains(
    before: &EventBoundary,
    after: &EventBoundary,
    producer: ProducerKind,
) -> bool {
    after
        .desired
        .iter()
        .filter(|required| required.producers.contains(producer))
        .all(|required| {
            required.path.ancestors().any(|ancestor| {
                let first = before
                    .desired
                    .partition_point(|existing| existing.path.as_path() < ancestor);
                before.desired[first..]
                    .iter()
                    .take_while(|existing| existing.path == ancestor)
                    .any(|existing| {
                        existing.producers.contains(producer) && existing.covers(required)
                    })
            })
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum WatchScope {
    Exact,
    Children,
    Recursive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ExistencePolicy {
    Required,
    Optional,
}

/// Which plain content changes one subscription admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ChangeAdmission {
    /// Every path the subscription's scope matches.
    Scope,
    /// Only a name with the content source extension; discovery still decides eligibility.
    ContentExtension,
}

impl ChangeAdmission {
    /// The admission that still admits every change either subscription admitted.
    fn widest(self, other: Self) -> Self {
        match (self, other) {
            (Self::Scope, _) | (_, Self::Scope) => Self::Scope,
            (Self::ContentExtension, Self::ContentExtension) => Self::ContentExtension,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WatchRequirement {
    path: PathBuf,
    scope: WatchScope,
    producers: ProducerSet,
    existence: ExistencePolicy,
    admission: ChangeAdmission,
    rebuild_scope: RebuildScope,
}

impl WatchRequirement {
    fn for_scope(path: PathBuf, scope: WatchScope, producers: ProducerSet) -> Self {
        // Discovery reads content members only through names with the source extension, so a
        // subscription the content producer owns admits a plain change through such a name. A
        // file a subscription names itself is never selected by a rule, so an exact scope admits
        // the file it registers.
        let admission = if scope != WatchScope::Exact && producers.contains(ProducerKind::Content) {
            ChangeAdmission::ContentExtension
        } else {
            ChangeAdmission::Scope
        };
        Self {
            path,
            scope,
            producers,
            existence: ExistencePolicy::Required,
            admission,
            rebuild_scope: RebuildScope::Paths,
        }
    }

    fn recursive_for(path: PathBuf, producers: ProducerSet) -> Self {
        Self::for_scope(path, WatchScope::Recursive, producers)
    }

    fn exact_for(path: PathBuf, producers: ProducerSet) -> Self {
        Self::for_scope(path, WatchScope::Exact, producers)
    }

    fn optional(mut self) -> Self {
        self.existence = ExistencePolicy::Optional;
        self
    }

    /// Whether a plain content change to `path` can affect this subscription.
    fn admits_change(&self, path: &Path) -> bool {
        match self.admission {
            ChangeAdmission::Scope => true,
            ChangeAdmission::ContentExtension => tola_build::has_typ_extension(path),
        }
    }

    /// Whether this subscription reads only the member names a discovery rule selects.
    ///
    /// Discovery selects content members by the source extension, so a name its rule does not
    /// match is never an input there. A subscription that names the file itself, or that admits
    /// every member of its scope — a configured asset tree, a configured font directory — reads
    /// what the scope holds whatever a member is called.
    fn selects_members_by_name(&self) -> bool {
        self.admission == ChangeAdmission::ContentExtension
    }

    fn with_rebuild_scope(mut self, rebuild_scope: RebuildScope) -> Self {
        self.rebuild_scope = rebuild_scope;
        self
    }

    fn matches(&self, path: &Path) -> bool {
        match self.scope {
            WatchScope::Exact => path == self.path,
            WatchScope::Children => path == self.path || path.parent() == Some(&self.path),
            WatchScope::Recursive => path.starts_with(&self.path),
        }
    }

    fn covers(&self, required: &Self) -> bool {
        match self.scope {
            WatchScope::Recursive => required.path.starts_with(&self.path),
            WatchScope::Children => {
                (self.path == required.path && required.scope != WatchScope::Recursive)
                    || (required.scope == WatchScope::Exact
                        && required.path.parent() == Some(&self.path))
            }
            WatchScope::Exact => self.path == required.path && required.scope == WatchScope::Exact,
        }
    }
}

fn requirements_for_configs<'a>(
    configs: impl IntoIterator<Item = &'a ResolvedSiteConfig>,
    dynamic: &WatchRequirements,
) -> Vec<WatchRequirement> {
    let mut desired = Vec::new();
    for config in configs {
        desired.extend(collect_roots(config, dynamic));
    }
    desired
}

struct WatchPathObservation {
    physical: PathBuf,
    entries: Vec<(PathBuf, Option<PathBuf>)>,
    parent: Option<PathBuf>,
    is_directory: Result<bool, std::io::ErrorKind>,
    unreadable: bool,
}

impl WatchPathObservation {
    fn capture(path: &Path) -> Self {
        let resolved = resolve_symlinks(path);
        let physical = normalize_existing_prefix(&resolved.physical);
        let entries = resolved
            .entries
            .into_iter()
            .map(|entry| {
                let entry = entry
                    .parent()
                    .zip(entry.file_name())
                    .map(|(parent, name)| normalize_existing_prefix(parent).join(name))
                    .unwrap_or(entry);
                let parent = entry.parent().and_then(nearest_existing_directory);
                (entry, parent)
            })
            .collect();
        let parent = physical.parent().and_then(nearest_existing_directory);
        let is_directory = std::fs::metadata(&physical)
            .map(|metadata| metadata.is_dir())
            .map_err(|error| error.kind());
        Self {
            physical,
            entries,
            parent,
            is_directory,
            unreadable: resolved.unreadable,
        }
    }
}

fn selection_for_observed(
    mut desired: Vec<WatchRequirement>,
    observed: &mut BTreeMap<PathBuf, WatchPathObservation>,
    hook_outputs: Arc<[tola_build::filesystem::FilesystemSourceIdentity]>,
) -> WatchSelection {
    dedupe_requirements(&mut desired);
    let mut matched = desired.clone();
    let mut directories = Vec::new();
    let mut directory_producers = BTreeMap::<PathBuf, ProducerSet>::new();
    let mut symlink_entries = Vec::new();
    let mut uncertainties = BTreeMap::new();
    for requirement in &desired {
        let resolved = observed
            .entry(requirement.path.clone())
            .or_insert_with(|| WatchPathObservation::capture(&requirement.path));
        if resolved.unreadable {
            record_uncertainty(
                &mut uncertainties,
                requirement.producers,
                CoverageUncertainty::Unreadable,
            );
        }
        let mut physical = requirement.clone();
        physical.path = resolved.physical.clone();
        matched.push(physical.clone());
        for (entry, parent) in &resolved.entries {
            symlink_entries.push(entry.clone());
            matched.push(
                WatchRequirement::exact_for(entry.clone(), requirement.producers)
                    .with_rebuild_scope(requirement.rebuild_scope)
                    .optional(),
            );
            if let Some(parent) = parent {
                add_directory(
                    &mut directories,
                    &mut directory_producers,
                    parent.clone(),
                    RecursiveMode::NonRecursive,
                    requirement.producers,
                );
            } else {
                record_uncertainty(
                    &mut uncertainties,
                    requirement.producers,
                    CoverageUncertainty::CoverageGap,
                );
            }
        }
        match resolved.is_directory {
            Ok(true) if requirement.scope != WatchScope::Exact => {
                let mode = if requirement.scope == WatchScope::Recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                add_directory(
                    &mut directories,
                    &mut directory_producers,
                    physical.path.clone(),
                    mode,
                    requirement.producers,
                );
                if let Some(parent) = &resolved.parent {
                    add_directory(
                        &mut directories,
                        &mut directory_producers,
                        parent.clone(),
                        RecursiveMode::NonRecursive,
                        requirement.producers,
                    );
                }
            }
            metadata => {
                match metadata {
                    Err(std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {
                        if requirement.existence == ExistencePolicy::Required {
                            record_uncertainty(
                                &mut uncertainties,
                                requirement.producers,
                                CoverageUncertainty::MissingRequired,
                            );
                        }
                    }
                    Err(_) => record_uncertainty(
                        &mut uncertainties,
                        requirement.producers,
                        CoverageUncertainty::Unreadable,
                    ),
                    Ok(_) => {}
                }
                if let Some(parent) = &resolved.parent {
                    add_directory(
                        &mut directories,
                        &mut directory_producers,
                        parent.clone(),
                        RecursiveMode::NonRecursive,
                        requirement.producers,
                    );
                } else {
                    record_uncertainty(
                        &mut uncertainties,
                        requirement.producers,
                        CoverageUncertainty::CoverageGap,
                    );
                }
            }
        }
    }
    minimize_directories(&mut directories);
    let directory_producers = directories
        .iter()
        .map(|directory| {
            let producers = if directory.mode == RecursiveMode::Recursive {
                directory_producers
                    .range(directory.path.clone()..)
                    .take_while(|(path, _)| path.starts_with(&directory.path))
                    .fold(ProducerSet::default(), |mut producers, (_, owners)| {
                        producers.insert(*owners);
                        producers
                    })
            } else {
                directory_producers
                    .get(&directory.path)
                    .copied()
                    .unwrap_or_default()
            };
            (directory.path.clone(), producers)
        })
        .collect();
    dedupe_requirements(&mut matched);
    symlink_entries.sort();
    symlink_entries.dedup();
    let producers = matched
        .iter()
        .fold(ProducerSet::default(), |mut producers, requirement| {
            producers.insert(requirement.producers);
            producers
        });
    let boundary = EventBoundary {
        producers,
        desired: matched.into(),
        directories: directories
            .iter()
            .map(|directory| directory.path.clone())
            .collect(),
        symlink_entries: symlink_entries.into(),
        hook_outputs,
    };
    WatchSelection {
        desired,
        boundary,
        directories,
        directory_producers,
        uncertainties,
    }
}

fn record_uncertainty(
    uncertainties: &mut BTreeMap<ProducerKind, CoverageUncertainty>,
    producers: ProducerSet,
    reason: CoverageUncertainty,
) {
    for producer in producers.iter() {
        uncertainties.entry(producer).or_insert(reason);
    }
}

fn add_directory(
    directories: &mut Vec<DirectoryWatch>,
    directory_producers: &mut BTreeMap<PathBuf, ProducerSet>,
    path: PathBuf,
    mode: RecursiveMode,
    producers: ProducerSet,
) {
    directory_producers
        .entry(path.clone())
        .or_default()
        .insert(producers);
    directories.push(DirectoryWatch { path, mode });
}

fn minimize_directories(directories: &mut Vec<DirectoryWatch>) {
    let mut modes = BTreeMap::new();
    for directory in directories.drain(..) {
        modes
            .entry(directory.path)
            .and_modify(|mode| {
                if directory.mode == RecursiveMode::Recursive {
                    *mode = RecursiveMode::Recursive;
                }
            })
            .or_insert(directory.mode);
    }
    let mut minimal: Vec<DirectoryWatch> = Vec::new();
    let mut recursive = BTreeSet::new();
    for (path, mode) in modes {
        if path
            .ancestors()
            .skip(1)
            .any(|ancestor| recursive.contains(ancestor))
        {
            continue;
        }
        if mode == RecursiveMode::Recursive {
            recursive.insert(path.clone());
        }
        minimal.push(DirectoryWatch { path, mode });
    }
    *directories = minimal;
}

struct ResolvedWatchPath {
    physical: PathBuf,
    entries: Vec<PathBuf>,
    unreadable: bool,
}

/// Keep link entries for retargeting and resolved paths for target creation, even when an
/// external target is missing.
fn resolve_symlinks(path: &Path) -> ResolvedWatchPath {
    let mut physical = lexical_path_identity(path);
    let mut entries = BTreeSet::new();
    loop {
        let mut prefix = PathBuf::new();
        let components = physical.components().collect::<Vec<_>>();
        let mut replacement = None;
        for (index, component) in components.iter().enumerate() {
            prefix.push(component.as_os_str());
            match std::fs::symlink_metadata(&prefix) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    if !entries.insert(prefix.clone()) {
                        return ResolvedWatchPath {
                            physical,
                            entries: entries.into_iter().collect(),
                            unreadable: true,
                        };
                    }
                    let Ok(target) = std::fs::read_link(&prefix) else {
                        return ResolvedWatchPath {
                            physical,
                            entries: entries.into_iter().collect(),
                            unreadable: true,
                        };
                    };
                    let mut rewritten = if target.is_absolute() {
                        target
                    } else {
                        prefix.parent().unwrap_or(Path::new("")).join(target)
                    };
                    for suffix in &components[index + 1..] {
                        rewritten.push(suffix.as_os_str());
                    }
                    replacement = Some(lexical_path_identity(&rewritten));
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    let unreadable = !matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    );
                    return ResolvedWatchPath {
                        physical,
                        entries: entries.into_iter().collect(),
                        unreadable,
                    };
                }
            }
        }
        if let Some(rewritten) = replacement {
            physical = rewritten;
        } else {
            return ResolvedWatchPath {
                physical,
                entries: entries.into_iter().collect(),
                unreadable: false,
            };
        }
    }
}

fn nearest_existing_directory(path: &Path) -> Option<PathBuf> {
    path.ancestors().find_map(|ancestor| {
        std::fs::metadata(ancestor)
            .ok()
            .filter(|metadata| metadata.is_dir())
            .and_then(|_| ancestor.canonicalize().ok())
    })
}

fn collect_roots(
    config: &ResolvedSiteConfig,
    dynamic: &WatchRequirements,
) -> Vec<WatchRequirement> {
    let content_producers =
        ProducerSet::one(ProducerKind::Content) | ProducerSet::one(ProducerKind::TypstReads);
    let mut roots = vec![
        WatchRequirement::recursive_for(
            resolved_config_path(&config.build().content_dir),
            content_producers,
        ),
        WatchRequirement::exact_for(
            resolved_config_path(&config.build().entry),
            ProducerSet::one(ProducerKind::TypstReads),
        ),
    ];
    // A configured font directory retains every accepted member with its bytes, so a member edit
    // reaches the build whatever the member is called.
    roots.extend(config.fonts().paths.iter().map(|path| {
        WatchRequirement::recursive_for(
            resolved_config_path(path),
            ProducerSet::one(ProducerKind::Fonts),
        )
        .optional()
    }));

    roots.extend(config.assets().tree_sources().map(|source| {
        WatchRequirement::recursive_for(
            resolved_config_path(source),
            ProducerSet::one(ProducerKind::ConfiguredAssets),
        )
        .optional()
    }));

    for source in config.assets().file_sources() {
        let source = resolved_config_path(source);
        roots.push(
            WatchRequirement::exact_for(source, ProducerSet::one(ProducerKind::ConfiguredAssets))
                .optional(),
        );
    }

    let configured = tola_build::build::configured_input_observation(config);
    let mut configured_requirements = WatchRequirements::default();
    crate::dev::watch::requirements::append_observed_paths(
        &mut configured_requirements,
        configured.paths(),
    );
    roots.extend(configured_requirements.iter().cloned());

    collect_hook_rerun_paths(config, &mut roots);

    roots.extend(dynamic.iter().cloned());

    if !config.config_path().as_os_str().is_empty() {
        roots.push(WatchRequirement::exact_for(
            resolved_config_path(config.config_path()),
            ProducerSet::ALL,
        ));
    }

    dedupe_requirements(&mut roots);
    roots
}

/// A configuration-declared path (hook declarations) resolves against the site root.
fn site_relative_path(root: &Path, declared: &Path) -> PathBuf {
    if declared.is_absolute() {
        lexical_path_identity(declared)
    } else {
        lexical_path_identity(&root.join(declared))
    }
}

fn resolved_config_path(path: &Path) -> PathBuf {
    lexical_path_identity(path)
}

/// Before-build hooks that participate in development builds, in declaration order.
pub(super) fn development_before_build_hooks(
    config: &ResolvedSiteConfig,
) -> impl Iterator<Item = &tola_build::config::section::build::BeforeBuildHookConfig> {
    config
        .build()
        .hooks
        .before_build
        .iter()
        .filter(|hook| hook.enable && hook.dev.participates_in_development())
}

/// Every output a development before-build hook declares, as a filesystem identity.
fn declared_hook_outputs(
    config: &ResolvedSiteConfig,
) -> Vec<tola_build::filesystem::FilesystemSourceIdentity> {
    development_before_build_hooks(config)
        .flat_map(|hook| hook.generates.iter())
        .map(|output| {
            tola_build::filesystem::FilesystemSourceIdentity::from_path(&site_relative_path(
                config.get_root(),
                output,
            ))
        })
        .collect()
}

/// Declared hook outputs of several observed configurations, deduplicated.
fn declared_hook_outputs_for(
    configs: &[&ResolvedSiteConfig],
) -> Vec<tola_build::filesystem::FilesystemSourceIdentity> {
    let mut outputs = Vec::new();
    for config in configs {
        for output in development_before_build_hooks(config).flat_map(|hook| hook.generates.iter())
        {
            outputs.push(tola_build::filesystem::FilesystemSourceIdentity::from_path(
                &site_relative_path(config.get_root(), output),
            ));
        }
    }
    outputs.sort_by(|left, right| left.logical_path().cmp(right.logical_path()));
    outputs.dedup_by(|left, right| left.logical_path() == right.logical_path());
    outputs
}

fn collect_hook_rerun_paths(config: &ResolvedSiteConfig, roots: &mut Vec<WatchRequirement>) {
    for hook in development_before_build_hooks(config) {
        for output in &hook.generates {
            let logical = site_relative_path(config.get_root(), output);
            let identity = tola_build::filesystem::FilesystemSourceIdentity::from_path(&logical);
            let paths = [
                identity.logical_path().to_path_buf(),
                identity.physical_path().to_path_buf(),
            ];
            let requirement = match std::fs::metadata(&logical) {
                Ok(metadata) if metadata.is_file() => paths.map(|path| {
                    WatchRequirement::exact_for(path, ProducerSet::one(ProducerKind::Hooks))
                        .optional()
                }),
                _ => paths.map(|path| {
                    WatchRequirement::recursive_for(path, ProducerSet::one(ProducerKind::Hooks))
                        .optional()
                }),
            };
            roots.extend(requirement);
        }
    }
    for path in config.build().hooks.development_rerun_paths() {
        roots.push(
            WatchRequirement::recursive_for(
                site_relative_path(config.get_root(), path),
                ProducerSet::one(ProducerKind::Hooks),
            )
            .optional(),
        );
    }
}

fn dedupe_requirements(requirements: &mut Vec<WatchRequirement>) {
    sort_requirements(requirements);
    let mut kept: Vec<WatchRequirement> = Vec::with_capacity(requirements.len());
    for requirement in requirements.drain(..) {
        if let Some(existing) = kept.last_mut().filter(|existing| {
            existing.path == requirement.path
                && existing.scope == requirement.scope
                && existing.existence == requirement.existence
        }) {
            existing.producers.insert(requirement.producers);
            existing.rebuild_scope = existing.rebuild_scope.max(requirement.rebuild_scope);
            // One merged subscription must still admit every change either of them admitted.
            existing.admission = existing.admission.widest(requirement.admission);
        } else {
            kept.push(requirement);
        }
    }
    *requirements = kept;
}

fn sort_requirements(requirements: &mut [WatchRequirement]) {
    requirements.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.existence.cmp(&right.existence))
            .then_with(|| left.rebuild_scope.cmp(&right.rebuild_scope))
    });
}

#[cfg(test)]
mod tests {
    use super::super::tests::{local_packages, resolve_schema};
    use super::{
        CoverageCertainty, CoverageUncertainty, DirectoryWatch, EventBoundary, ExistencePolicy,
        ProducerKind, ProducerSet, RootSet, WatchRequirement, WatchRequirements, WatchScope,
        collect_roots,
    };
    use crate::dev::watch::fs::RebuildScope;
    use notify::RecursiveMode;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tola_build::config::ResolvedSiteConfig;

    #[test]
    fn indexed_matching_agrees_with_full_subscription_scan() {
        use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind};
        let mut desired = Vec::new();
        for prefix in [
            "", "site", "site/a", "site/a/b", "site/a-b", "site/aa", "other",
        ] {
            for scope in [
                WatchScope::Exact,
                WatchScope::Children,
                WatchScope::Recursive,
            ] {
                for producer in [
                    ProducerKind::Content,
                    ProducerKind::Fonts,
                    ProducerKind::Hooks,
                ] {
                    desired.push(WatchRequirement::for_scope(
                        PathBuf::from(prefix),
                        scope,
                        ProducerSet::one(producer),
                    ));
                }
            }
        }
        // Exercise each admission/scope independently too: a broad subscription in the
        // combined set must not hide an incorrectly accepted narrower subscription.
        let mut cases = desired
            .iter()
            .cloned()
            .map(|requirement| vec![requirement])
            .collect::<Vec<_>>();
        cases.push(desired);
        for mut desired in cases {
            super::dedupe_requirements(&mut desired);
            let boundary = EventBoundary {
                desired: desired.into(),
                ..Default::default()
            };
            for path in [
                "",
                "site",
                "site/a",
                "site/a/b",
                "site/a/b/c.typ",
                "site/a/b/c.css",
                "site/a-b/file.typ",
                "site/aa/file.typ",
                "other/file.typ",
                "unrelated",
            ] {
                let path = Path::new(path);
                for kind in [
                    notify::EventKind::Any,
                    notify::EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                    notify::EventKind::Create(CreateKind::File),
                    notify::EventKind::Remove(RemoveKind::Folder),
                ] {
                    let mut expected = super::ClassifiedPath::default();
                    for requirement in boundary.desired.iter() {
                        if requirement.path.starts_with(path)
                            || (requirement.matches(path)
                                && (super::is_structural(kind) || requirement.admits_change(path)))
                        {
                            expected.merge(requirement, false);
                        }
                    }
                    assert_eq!(
                        boundary.classify_change(path, kind),
                        expected,
                        "{path:?} {kind:?}"
                    );
                    let expected_artifact = boundary.desired.iter().any(|requirement| {
                        requirement.path.starts_with(path)
                            || (requirement.matches(path) && !requirement.selects_members_by_name())
                    });
                    assert_eq!(boundary.admits_editor_artifact(path), expected_artifact);
                }
            }
        }
    }

    #[test]
    fn event_lookup_visits_only_intersecting_subscriptions() {
        let mut desired = (0..10_000)
            .map(|index| {
                exact(
                    PathBuf::from(format!("site/content/{index:05}.typ")),
                    ProducerKind::TypstReads,
                )
            })
            .collect::<Vec<_>>();
        desired.push(recursive(
            PathBuf::from("site/content"),
            ProducerKind::Content,
        ));
        super::dedupe_requirements(&mut desired);
        let boundary = EventBoundary {
            desired: desired.into(),
            ..Default::default()
        };
        assert_eq!(
            boundary
                .intersecting_requirements(Path::new("site/content/05000.typ"))
                .count(),
            2
        );
        assert_eq!(
            boundary
                .intersecting_requirements(Path::new("site/content/unknown.typ"))
                .count(),
            1
        );
        assert_eq!(
            boundary
                .intersecting_requirements(Path::new("site/content"))
                .count(),
            10_001
        );
        assert_eq!(
            boundary
                .intersecting_requirements(Path::new("site/content-backup"))
                .count(),
            0
        );
    }

    fn configured_site(root: &Path, source: &str) -> ResolvedSiteConfig {
        let schema = tola_build::config::SiteConfigSchema::parse(
            &root.join("tola.toml"),
            source,
            tola_build::InputScope::Online,
        )
        .unwrap();
        resolve_schema(root, schema, local_packages())
    }

    /// A temporary site with the default configuration and its content directory created.
    fn default_site() -> (TempDir, PathBuf, ResolvedSiteConfig) {
        let (directory, root) = canonical_directory();
        std::fs::create_dir_all(root.join("content")).unwrap();
        let config = configured_site(&root, "");
        (directory, root, config)
    }

    fn has_requirement(
        requirements: &[WatchRequirement],
        path: &Path,
        scope: WatchScope,
        producer: ProducerKind,
        existence: ExistencePolicy,
    ) -> bool {
        requirements.iter().any(|requirement| {
            requirement.path == path
                && requirement.scope == scope
                && requirement.producers.contains(producer)
                && requirement.existence == existence
        })
    }

    fn canonical_directory() -> (TempDir, PathBuf) {
        let directory = TempDir::new().unwrap();
        let path = directory.path().canonicalize().unwrap();
        (directory, path)
    }

    fn exact(path: PathBuf, producer: ProducerKind) -> WatchRequirement {
        WatchRequirement::exact_for(path, ProducerSet::one(producer))
    }

    fn recursive(path: PathBuf, producer: ProducerKind) -> WatchRequirement {
        WatchRequirement::recursive_for(path, ProducerSet::one(producer))
    }

    fn selection_for(desired: Vec<WatchRequirement>) -> super::WatchSelection {
        super::selection_for_observed(desired, &mut BTreeMap::new(), Arc::default())
    }

    fn root_set(desired: Vec<WatchRequirement>) -> RootSet {
        RootSet::with_hook_outputs(desired, Arc::default())
    }

    fn install(roots: &mut RootSet, selection: super::WatchSelection, epoch: u64) {
        let update = roots.directory_update(&selection, false);
        roots.commit(selection, update, epoch);
    }

    fn watches(selection: &super::WatchSelection, path: &Path, mode: RecursiveMode) -> bool {
        selection
            .directories
            .iter()
            .any(|directory| directory.path() == path && directory.recursive_mode() == mode)
    }

    fn accepts(boundary: &EventBoundary, path: &Path) -> bool {
        !boundary.classify(path).is_empty()
    }

    #[test]
    fn output_root_is_never_watched() {
        let (_directory, root, config) = default_site();
        let output = root.join("public");

        let roots = collect_roots(&config, &WatchRequirements::default());

        assert!(!output.exists());
        assert!(!roots.iter().any(|watch| watch.path == output));
    }

    #[test]
    fn icon_sources_keep_recovery_coverage() {
        let directory = TempDir::new().unwrap();
        let root = tola_build::filesystem::normalize_path(directory.path());
        let content = root.join("content");
        std::fs::create_dir_all(&content).unwrap();
        let collection = root.join("missing/icons.json");
        let drawings = root.join("missing/drawings");
        let mut schema = tola_build::config::SiteConfigSchema::default();
        schema.icons.collections.insert(
            "ui".into(),
            tola_build::config::section::IconCollectionSource::LocalJson {
                path: PathBuf::from("missing/icons.json"),
            },
        );
        schema.icons.collections.insert(
            "brand".into(),
            tola_build::config::section::IconCollectionSource::LocalSvgDir {
                path: PathBuf::from("missing/drawings"),
            },
        );
        let config = resolve_schema(&root, schema, local_packages());
        let roots = collect_roots(&config, &WatchRequirements::default());
        assert!(has_requirement(
            &roots,
            &collection,
            WatchScope::Exact,
            ProducerKind::Icons,
            ExistencePolicy::Optional
        ));
        assert!(has_requirement(
            &roots,
            &drawings,
            WatchScope::Recursive,
            ProducerKind::Icons,
            ExistencePolicy::Optional
        ));
    }

    #[test]
    fn site_entry_is_watched_exactly() {
        let (_directory, root, config) = default_site();
        let entry = root.join("site.typ");
        std::fs::write(&entry, "#document(\"index.html\")[Hello]").unwrap();

        let roots = collect_roots(&config, &WatchRequirements::default());

        assert!(has_requirement(
            &roots,
            &entry,
            WatchScope::Exact,
            ProducerKind::TypstReads,
            ExistencePolicy::Required,
        ));
    }

    #[test]
    fn font_root_is_recursive_input() {
        let directory = TempDir::new().unwrap();
        let root = &directory.path().canonicalize().unwrap();
        let content = root.join("content");
        let fonts = root.join("fonts");
        std::fs::create_dir_all(&content).unwrap();
        let config = configured_site(root, "[typst.fonts]\npaths = [\"fonts\"]\n");

        let roots = collect_roots(&config, &WatchRequirements::default());
        let selection = selection_for(roots);
        let boundary = selection.boundary();

        assert!(has_requirement(
            &selection.desired,
            &fonts,
            WatchScope::Recursive,
            ProducerKind::Fonts,
            ExistencePolicy::Optional,
        ));
        assert!(accepts(&boundary, &fonts));
        assert!(accepts(&boundary, &fonts.join("site.woff2")));
    }

    #[test]
    fn typst_recovery_needs_full_rebuild() {
        let (_directory, root, config) = default_site();
        let mut dynamic = WatchRequirements::default();
        dynamic.typst_recovery(root.to_path_buf());

        let selection = selection_for(collect_roots(&config, &dynamic));
        let classified = selection.boundary().classify(&root.join("support.typ"));

        assert!(classified.producers().contains(ProducerKind::TypstReads));
        assert_eq!(
            classified.rebuild_scope(),
            crate::dev::watch::fs::RebuildScope::FullTypst
        );
    }

    #[test]
    fn package_checks_track_availability() {
        let (_directory, root, config) = default_site();
        let data = root.join("package-data");
        let cache = root.join("package-cache");
        let missing = data.join("local/example/1.0.0");
        let selected = cache.join("local/example/1.0.0");
        std::fs::create_dir_all(&selected).unwrap();
        let locations =
            tola_typst::PackageLocations::from_absolute_roots(Some(data), Some(cache)).unwrap();
        let store =
            tola_typst::PackageStore::new(locations, tola_typst::PackageFetchPolicy::LocalOnly);
        let package = "@local/example:1.0.0".parse().unwrap();
        let prepared = store.prepare(&package).unwrap();
        let mut dynamic = WatchRequirements::default();
        dynamic.package_checks(prepared.checks().iter().cloned());

        let selection = selection_for(collect_roots(&config, &dynamic));
        let boundary = selection.boundary();

        assert!(has_requirement(
            &selection.desired,
            &missing,
            WatchScope::Exact,
            ProducerKind::PackageResolution,
            ExistencePolicy::Optional,
        ));
        assert!(has_requirement(
            &selection.desired,
            &selected,
            WatchScope::Exact,
            ProducerKind::PackageResolution,
            ExistencePolicy::Required,
        ));
        assert_eq!(
            boundary.classify(&missing).rebuild_scope(),
            crate::dev::watch::fs::RebuildScope::FullTypst
        );
        assert_eq!(
            boundary.classify(&selected).rebuild_scope(),
            crate::dev::watch::fs::RebuildScope::FullTypst
        );
    }

    #[test]
    fn package_reads_stay_optional() {
        let (_directory, root, config) = default_site();
        let package_file = root.join(".tola/builtin-packages/local/example/1.0.0/lib.typ");
        let mut dynamic = WatchRequirements::default();
        dynamic.package_files([package_file.clone()]);

        let selection = selection_for(collect_roots(&config, &dynamic));
        let classified = selection.boundary().classify(&package_file);

        assert!(has_requirement(
            &selection.desired,
            &package_file,
            WatchScope::Exact,
            ProducerKind::PackageResolution,
            ExistencePolicy::Optional,
        ));
        assert!(
            classified
                .producers()
                .contains(ProducerKind::PackageResolution)
        );
        assert_eq!(
            classified.rebuild_scope(),
            crate::dev::watch::fs::RebuildScope::FullTypst
        );
    }

    #[test]
    fn missing_asset_sources_stay_watched() {
        let temp = TempDir::new().unwrap();
        let config = configured_site(
            temp.path(),
            "[assets]\ntrees = [{ source = \"static/web-assets\", url-prefix = \"/assets\" }]\n",
        );
        let root = config.get_root().to_path_buf();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::create_dir_all(root.join("public")).unwrap();

        let roots = collect_roots(&config, &WatchRequirements::default());
        assert!(has_requirement(
            &roots,
            &root.join("static/web-assets"),
            WatchScope::Recursive,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Optional,
        ));
    }

    #[test]
    fn physical_reads_watch_exact_files() {
        let (_directory, root, config) = default_site();
        let templates = root.join("theme/templates");
        std::fs::create_dir_all(&templates).unwrap();
        let template = templates.join("document.typ");
        std::fs::write(&template, "#let document-template(body) = body").unwrap();

        let mut dynamic = WatchRequirements::default();
        dynamic.exact(ProducerKind::TypstReads, [template.clone()]);
        let roots = collect_roots(&config, &dynamic);

        assert!(has_requirement(
            &roots,
            &template,
            WatchScope::Exact,
            ProducerKind::TypstReads,
            ExistencePolicy::Required,
        ));
        assert!(!roots.iter().any(|watch| {
            watch.path == root
                && watch.scope == WatchScope::Recursive
                && watch.producers.contains(ProducerKind::TypstReads)
        }));
    }

    #[test]
    fn producer_policy_keeps_directories() {
        let (_directory, root) = canonical_directory();
        let file = root.join("shared.json");
        std::fs::write(&file, "{}").unwrap();
        let required = selection_for(vec![exact(file.clone(), ProducerKind::TypstReads)]);
        let optional = selection_for(vec![
            exact(file.clone(), ProducerKind::Fonts)
                .optional()
                .with_rebuild_scope(RebuildScope::FullTypst),
            exact(file, ProducerKind::ConfiguredAssets),
        ]);
        assert_eq!(required.directories, optional.directories);
        assert_eq!(
            required.directories,
            vec![DirectoryWatch {
                path: root,
                mode: RecursiveMode::NonRecursive
            }]
        );
    }

    #[test]
    fn changes_reach_subscriptions_by_kind_and_name() {
        use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

        let temp = TempDir::new().unwrap();
        let root = &temp.path().canonicalize().unwrap();
        let config = configured_site(
            root,
            "[assets]\ntrees = [{ source = \"static\", url-prefix = \"/assets\" }]\n\
             [[build.hooks.before-build]]\nname = \"prepare\"\ncommand = [\"true\"]\n\
             rerun-on = [\"scripts\"]\n",
        );
        let content = root.join("content");
        let static_tree = root.join("static");
        let scripts = root.join("scripts");
        let read = root.join("inputs/data.json");
        let content_read = content.join("data.json");
        for directory in [&content, &static_tree, &scripts] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::create_dir_all(read.parent().unwrap()).unwrap();
        let mut dynamic = WatchRequirements::default();
        dynamic.exact(ProducerKind::TypstReads, [read.clone()]);
        dynamic.exact(ProducerKind::TypstReads, [content_read.clone()]);
        let boundary = selection_for(collect_roots(&config, &dynamic)).boundary();

        let content_root =
            ProducerSet::one(ProducerKind::Content) | ProducerSet::one(ProducerKind::TypstReads);
        let reads = ProducerSet::one(ProducerKind::TypstReads);
        let data_change = || notify::EventKind::Modify(ModifyKind::Data(DataChange::Any));
        let file_create = || notify::EventKind::Create(CreateKind::File);
        let file_remove = || notify::EventKind::Remove(RemoveKind::File);
        let cases = [
            // A name discovery never consumes stops waking the session.
            (
                content.join("extra.txt"),
                data_change(),
                ProducerSet::default(),
            ),
            (
                content.join("image.png"),
                data_change(),
                ProducerSet::default(),
            ),
            // A file-level create or remove has that file's own name, which nothing moves
            // under, so only the subscriptions the name earns wake; macOS reports an ordinary
            // write to an existing file as a create.
            (
                content.join("extra.txt"),
                file_create(),
                ProducerSet::default(),
            ),
            (
                content.join("extra.txt"),
                file_remove(),
                ProducerSet::default(),
            ),
            // Every name discovery consumes still reaches the content producers.
            (content.join("page.typ"), data_change(), content_root),
            (content.join("page.typ"), file_create(), content_root),
            // The root path itself reports the tree, so a plain change to it keeps the scope.
            (content.clone(), data_change(), content_root),
            // A created, removed, or renamed tree keeps the whole scope: its path has no
            // extension and its subtree reports nothing.
            (
                content.join("nested"),
                notify::EventKind::Create(CreateKind::Folder),
                content_root,
            ),
            (
                content.join("nested"),
                notify::EventKind::Remove(RemoveKind::Folder),
                content_root,
            ),
            (
                content.join("page.typ"),
                notify::EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                content_root,
            ),
            // An event that has no kind reaches the conservative scope too.
            (
                content.join("extra.txt"),
                notify::EventKind::Any,
                content_root,
            ),
            // Independent subscriptions are not shadowed by the content root's admission.
            (read.clone(), data_change(), reads),
            (read.clone(), file_create(), reads),
            // A file the build reads inside the narrowed root keeps its own subscription.
            (content_read.clone(), data_change(), reads),
            (content_read.clone(), file_create(), reads),
            (content_read.clone(), file_remove(), reads),
            (
                scripts.join("search.mjs"),
                data_change(),
                ProducerSet::one(ProducerKind::Hooks),
            ),
            (
                static_tree.join("app.css"),
                data_change(),
                ProducerSet::one(ProducerKind::ConfiguredAssets),
            ),
        ];
        for (path, kind, expected) in cases {
            assert_eq!(
                boundary.classify_change(&path, kind).producers(),
                expected,
                "{} under {kind:?}",
                path.display()
            );
        }
    }

    #[test]
    fn merged_subscriptions_keep_the_wider_admission() {
        let (_directory, root, config) = default_site();
        let content = root.join("content");
        let mut dynamic = WatchRequirements::default();
        dynamic.recursive(ProducerKind::Hooks, [content.clone()]);
        let boundary = selection_for(collect_roots(&config, &dynamic)).boundary();
        let change = notify::EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Any,
        ));

        assert_eq!(
            boundary
                .classify_change(&content.join("data.json"), change)
                .producers(),
            ProducerSet::one(ProducerKind::Content)
                | ProducerSet::one(ProducerKind::TypstReads)
                | ProducerSet::one(ProducerKind::Hooks),
            "a merged subscription admits what its wider half admitted"
        );
    }

    #[test]
    fn recursive_parent_covers_new_inputs() {
        let (_directory, root) = canonical_directory();
        let content = root.join("content");
        let nested = content.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let mut roots = root_set(vec![recursive(content.clone(), ProducerKind::Content)]);
        let initial = roots.initial_selection();
        install(&mut roots, initial, 1);
        roots.confirm_current_observation();
        let mut desired = roots.selection.desired.clone();
        for (producer, name) in [
            (ProducerKind::TypstReads, "theme.typ"),
            (ProducerKind::Fonts, "theme-font.woff2"),
            (ProducerKind::ConfiguredAssets, "image.svg"),
        ] {
            let file = nested.join(name);
            std::fs::write(&file, "input").unwrap();
            desired.push(exact(file, producer));
        }
        let selected = selection_for(desired);
        assert!(
            roots.extends_observation(&selected),
            "the added producers need their first observation"
        );
        assert!(
            roots
                .directory_update(&selected, false)
                .retains_directories()
        );
        install(&mut roots, selected, 2);
        assert!(
            roots
                .coverage(ProducerKind::Content)
                .unwrap()
                .covers_current_revision()
        );
        for producer in [
            ProducerKind::TypstReads,
            ProducerKind::Fonts,
            ProducerKind::ConfiguredAssets,
        ] {
            assert_eq!(
                roots.coverage(producer).unwrap().certainty(),
                CoverageCertainty::AcceptedEvents
            );
            assert!(!roots.coverage(producer).unwrap().covers_current_revision());
        }
        assert_eq!(
            roots
                .boundary()
                .classify(&nested.join("theme-font.woff2"))
                .producers(),
            ProducerSet::one(ProducerKind::Content) | ProducerSet::one(ProducerKind::Fonts)
        );
        assert_eq!(
            roots
                .boundary()
                .classify(&content.join("sibling.typ"))
                .producers(),
            ProducerSet::one(ProducerKind::Content)
        );
    }

    #[test]
    fn redundant_read_keeps_observation() {
        let (_directory, root) = canonical_directory();
        let content = root.join("content");
        std::fs::create_dir_all(&content).unwrap();
        let mut roots = root_set(vec![recursive(content.clone(), ProducerKind::TypstReads)]);
        let selected = roots.initial_selection();
        install(&mut roots, selected, 4);
        roots.confirm_current_observation();
        let mut desired = roots.selection.desired.clone();
        desired.push(exact(content.join("new.typ"), ProducerKind::TypstReads).optional());
        let selected = selection_for(desired);
        assert!(!roots.extends_observation(&selected));
        assert!(
            roots
                .directory_update(&selected, false)
                .retains_directories()
        );
        install(&mut roots, selected, 7);
        let coverage = roots.coverage(ProducerKind::TypstReads).unwrap();
        assert_eq!(coverage.established_epoch(), 4);
        assert!(coverage.covers_current_revision());
    }

    #[test]
    fn parent_coverage_survives_replacement() {
        let (_directory, root) = canonical_directory();
        let file = root.join("one.typ");
        let mut roots = root_set(vec![
            exact(file.clone(), ProducerKind::TypstReads).optional(),
        ]);
        let selected = roots.initial_selection();
        install(&mut roots, selected, 1);
        std::fs::write(&file, "first").unwrap();
        let created = roots.current_selection();
        assert!(
            roots
                .directory_update(&created, false)
                .retains_directories()
        );
        let replacement = root.join("replacement");
        std::fs::write(&replacement, "second").unwrap();
        std::fs::rename(&replacement, &file).unwrap();
        let replaced = roots.current_selection();
        assert!(
            roots
                .directory_update(&replaced, false)
                .retains_directories()
        );
        let event = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(file.clone());
        assert!(roots.boundary().invalidated_by(&event).is_empty());
        assert!(accepts(&roots.boundary(), &file));
        assert!(!accepts(&roots.boundary(), &root.join("unrelated.typ")));
    }

    #[test]
    fn shrink_keeps_installed_directories() {
        let (_directory, root) = canonical_directory();
        let a = root.join("a");
        let b = root.join("b");
        let c = root.join("c");
        for path in [&a, &b, &c] {
            std::fs::create_dir_all(path).unwrap();
        }
        let first = recursive(a.clone(), ProducerKind::Content);
        let second = recursive(b.clone(), ProducerKind::Content);
        let third = recursive(c.clone(), ProducerKind::Content);
        let mut roots = root_set(vec![first.clone()]);
        let selected = roots.initial_selection();
        install(&mut roots, selected, 1);
        install(&mut roots, selection_for(vec![first, second.clone()]), 2);
        let installed = roots.installed.clone();
        let published = selection_for(vec![second.clone()]);
        assert!(!roots.extends_observation(&published));
        assert!(
            roots
                .directory_update(&published, false)
                .retains_directories()
        );
        roots.commit_logical(published, 3);
        assert_eq!(roots.installed, installed);
        assert!(!accepts(&roots.boundary(), &a.join("left.typ")));
        let next = selection_for(vec![second, third]);
        let update = roots.directory_update(&next, false);
        assert!(!update.retains_directories());
        roots.commit(next, update, 4);
        assert!(roots.installed.iter().any(|directory| directory.path == b));
        assert!(roots.installed.iter().any(|directory| directory.path == c));
        assert!(!roots.installed.iter().any(|directory| directory.path == a));
    }

    #[test]
    fn retained_directory_loss_invalidates() {
        let (_directory, root) = canonical_directory();
        let a = root.join("a");
        let b = root.join("b");
        for path in [&a, &b] {
            std::fs::create_dir_all(path).unwrap();
        }
        let mut roots = root_set(vec![
            recursive(a.clone(), ProducerKind::Content),
            recursive(b.clone(), ProducerKind::Content),
        ]);
        let selected = roots.initial_selection();
        install(&mut roots, selected, 1);
        roots.commit_logical(selection_for(vec![recursive(b, ProducerKind::Content)]), 2);
        assert!(!accepts(&roots.boundary(), &a));
        let child = notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::File))
            .add_path(a.join("unused.typ"));
        assert!(roots.boundary().invalidated_by(&child).is_empty());
        let removed =
            notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Folder))
                .add_path(a);
        assert_eq!(roots.boundary().invalidated_by(&removed), ProducerSet::ALL);
    }

    #[test]
    fn wider_scope_extends_observation() {
        let (_directory, root) = canonical_directory();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let mut dynamic = WatchRequirements::default();
        dynamic.optional_children(ProducerKind::Fonts, [source.clone()]);
        let mut roots = root_set(dynamic.iter().cloned().collect());
        let selected = roots.initial_selection();
        install(&mut roots, selected, 1);
        roots.confirm_current_observation();
        let recursive = selection_for(vec![recursive(source.clone(), ProducerKind::Fonts)]);
        assert!(roots.extends_observation(&recursive));
        let mut update = roots.directory_update(&recursive, false);
        assert!(update.take_operations().iter().any(|operation| matches!(operation,
            notify::PathOp::Watch(path, options) if *path == source && options.recursive_mode() == RecursiveMode::Recursive)));
        roots.commit(recursive, update, 2);
        assert_eq!(
            roots
                .coverage(ProducerKind::Fonts)
                .unwrap()
                .established_epoch(),
            2
        );
        assert!(
            !roots
                .coverage(ProducerKind::Fonts)
                .unwrap()
                .covers_current_revision()
        );
    }

    #[test]
    fn missing_paths_use_nearest_parent() {
        let (_directory, root) = canonical_directory();
        let target = root.join("generated/fonts");
        let mut roots = root_set(vec![
            recursive(target.clone(), ProducerKind::Fonts).optional(),
        ]);
        let missing = roots.initial_selection();
        assert_eq!(
            missing.directories,
            vec![DirectoryWatch {
                path: root.clone(),
                mode: RecursiveMode::NonRecursive
            }]
        );
        assert!(accepts(&missing.boundary(), &root.join("generated")));
        assert!(!accepts(&missing.boundary(), &root.join("unrelated")));
        install(&mut roots, missing, 1);
        std::fs::create_dir(root.join("generated")).unwrap();
        let intermediate = roots.current_selection();
        assert!(watches(
            &intermediate,
            &root.join("generated"),
            RecursiveMode::NonRecursive
        ));
        install(&mut roots, intermediate, 2);
        std::fs::create_dir(&target).unwrap();
        let existing = roots.current_selection();
        assert!(watches(&existing, &target, RecursiveMode::Recursive));
        assert!(
            !roots
                .directory_update(&existing, false)
                .retains_directories()
        );
    }

    #[test]
    fn covered_missing_paths_stay_narrow() {
        let (_directory, root) = canonical_directory();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let target = source.join("generated/fonts");
        let mut roots = root_set(vec![
            recursive(source.clone(), ProducerKind::Content),
            recursive(target.clone(), ProducerKind::Fonts).optional(),
        ]);
        let missing = roots.initial_selection();
        install(&mut roots, missing, 1);
        std::fs::create_dir_all(&target).unwrap();
        let existing = roots.current_selection();
        assert!(
            roots
                .directory_update(&existing, false)
                .retains_directories()
        );
        assert!(accepts(&existing.boundary(), &target.join("font.woff2")));
    }

    #[test]
    fn external_file_watches_its_parent() {
        let (_directory, root) = canonical_directory();
        let package = root.join("cache/local/library/1.0.0");
        std::fs::create_dir_all(&package).unwrap();
        let file = package.join("lib.typ");
        std::fs::write(&file, "#let value = 1").unwrap();
        let selected = selection_for(vec![exact(file.clone(), ProducerKind::PackageResolution)]);
        assert_eq!(
            selected.directories,
            vec![DirectoryWatch {
                path: package,
                mode: RecursiveMode::NonRecursive
            }]
        );
        assert!(accepts(&selected.boundary(), &file));
        assert!(!accepts(
            &selected.boundary(),
            &root.join("cache/local/unrelated/1.0.0/lib.typ")
        ));
    }

    #[test]
    fn install_clears_coverage_gap() {
        let (_directory, root) = canonical_directory();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let mut roots = root_set(Vec::new());
        let selection = selection_for(vec![recursive(source, ProducerKind::Content)]);
        let update = roots.directory_update(&selection, false);
        roots.commit_logical(selection.clone(), 3);
        roots.confirm_current_observation();
        let pending = roots.coverage(ProducerKind::Content).unwrap();
        assert_eq!(
            pending.certainty(),
            CoverageCertainty::Uncertain(CoverageUncertainty::CoverageGap)
        );
        assert!(!pending.covers_current_revision());
        assert!(roots.installed.is_empty());
        roots.commit(selection, update, 4);
        assert_eq!(
            roots.coverage(ProducerKind::Content).unwrap().certainty(),
            CoverageCertainty::AcceptedEvents
        );
        roots.confirm_current_observation();
        assert!(
            roots
                .coverage(ProducerKind::Content)
                .unwrap()
                .covers_current_revision()
        );
    }

    #[test]
    fn policy_changes_keep_observation() {
        let (_directory, root) = canonical_directory();
        let file = root.join("optional.typ");
        let mut roots = root_set(vec![exact(file.clone(), ProducerKind::TypstReads)]);
        let selected = roots.initial_selection();
        install(&mut roots, selected, 1);
        assert_eq!(
            roots
                .coverage(ProducerKind::TypstReads)
                .unwrap()
                .certainty(),
            CoverageCertainty::Uncertain(CoverageUncertainty::MissingRequired)
        );
        let optional = selection_for(vec![
            exact(file, ProducerKind::TypstReads)
                .optional()
                .with_rebuild_scope(RebuildScope::FullTypst),
        ]);
        assert!(!roots.extends_observation(&optional));
        assert!(
            roots
                .directory_update(&optional, false)
                .retains_directories()
        );
        install(&mut roots, optional, 2);
        assert_eq!(
            roots
                .coverage(ProducerKind::TypstReads)
                .unwrap()
                .certainty(),
            CoverageCertainty::AcceptedEvents
        );
    }

    #[test]
    fn refresh_reestablishes_observation() {
        let (_directory, root) = canonical_directory();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let mut roots = root_set(vec![recursive(source, ProducerKind::Content)]);
        let selected = roots.initial_selection();
        install(&mut roots, selected, 1);
        roots.confirm_current_observation();
        let selected = roots.current_selection();
        let mut update = roots.directory_update(&selected, true);
        assert!(!update.take_operations().is_empty());
        assert!(!update.retains_directories());
        roots.commit(selected, update, 5);
        assert_eq!(
            roots
                .coverage(ProducerKind::Content)
                .unwrap()
                .established_epoch(),
            5
        );
        assert!(
            !roots
                .coverage(ProducerKind::Content)
                .unwrap()
                .covers_current_revision()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entries_track_their_targets() {
        use std::os::unix::fs::symlink;
        let (_directory, root) = canonical_directory();
        let site = root.join("site");
        let first = root.join("first");
        let second = root.join("second");
        for path in [&site, &first, &second] {
            std::fs::create_dir_all(path).unwrap();
        }
        let link = site.join("theme");
        symlink(&first, &link).unwrap();
        let mut roots = root_set(vec![recursive(link.clone(), ProducerKind::TypstReads)]);
        let selected = roots.initial_selection();
        assert!(watches(&selected, &first, RecursiveMode::Recursive));
        assert!(watches(&selected, &site, RecursiveMode::NonRecursive));
        assert!(accepts(&selected.boundary(), &first.join("lib.typ")));
        assert!(accepts(&selected.boundary(), &link));
        assert!(!accepts(&selected.boundary(), &site.join("other")));
        install(&mut roots, selected, 1);
        std::fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        let retarget =
            notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::Any))
                .add_path(link);
        assert_eq!(roots.boundary().invalidated_by(&retarget), ProducerSet::ALL);
        let selected = roots.current_selection();
        assert!(roots.extends_observation(&selected));
        assert!(watches(&selected, &second, RecursiveMode::Recursive));
        assert!(
            !roots
                .directory_update(&selected, false)
                .retains_directories()
        );
        install(&mut roots, selected, 2);
        assert!(accepts(&roots.boundary(), &second.join("lib.typ")));
        assert!(!accepts(&roots.boundary(), &first.join("lib.typ")));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_watches_its_target() {
        use std::os::unix::fs::symlink;
        let (_directory, root) = canonical_directory();
        let site = root.join("site");
        let external = root.join("external");
        for path in [&site, &external] {
            std::fs::create_dir_all(path).unwrap();
        }
        let target = external.join("generated/templates");
        let link = site.join("theme");
        symlink(&target, &link).unwrap();
        let selected = selection_for(vec![
            recursive(link.clone(), ProducerKind::Fonts).optional(),
        ]);
        assert!(watches(&selected, &site, RecursiveMode::NonRecursive));
        assert!(watches(&selected, &external, RecursiveMode::NonRecursive));
        assert!(accepts(&selected.boundary(), &external.join("generated")));
        assert!(accepts(&selected.boundary(), &target));
        assert!(!accepts(&selected.boundary(), &external.join("unrelated")));
        assert_eq!(selected.uncertainties.get(&ProducerKind::Fonts), None);
    }

    #[cfg(unix)]
    #[test]
    fn intermediate_symlinks_watch_entries() {
        use std::os::unix::fs::symlink;
        let (_directory, root) = canonical_directory();
        let site = root.join("site");
        let package = root.join("package");
        let templates = root.join("templates");
        for path in [&site, &package, &templates] {
            std::fs::create_dir_all(path).unwrap();
        }
        let first = site.join("theme");
        let second = package.join("src");
        symlink(&package, &first).unwrap();
        symlink(&templates, &second).unwrap();
        let file = templates.join("lib.typ");
        std::fs::write(&file, "#let value = 1").unwrap();
        let selected = selection_for(vec![exact(
            first.join("src/lib.typ"),
            ProducerKind::TypstReads,
        )]);
        for parent in [&site, &package, &templates] {
            assert!(watches(&selected, parent, RecursiveMode::NonRecursive));
        }
        for entry in [first, second] {
            let event =
                notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Any))
                    .add_path(entry.clone());
            assert!(accepts(&selected.boundary(), &entry));
            assert_eq!(selected.boundary().invalidated_by(&event), ProducerSet::ALL);
        }
        assert!(accepts(&selected.boundary(), &file));
    }

    #[cfg(unix)]
    #[test]
    fn cyclic_symlink_is_uncertain() {
        use std::os::unix::fs::symlink;
        let (_directory, root) = canonical_directory();
        let link = root.join("theme");
        symlink("theme", &link).unwrap();
        let selected = selection_for(vec![recursive(link.clone(), ProducerKind::TypstReads)]);
        assert_eq!(
            selected.uncertainties.get(&ProducerKind::TypstReads),
            Some(&CoverageUncertainty::Unreadable)
        );
        assert!(watches(&selected, &root, RecursiveMode::NonRecursive));
        assert!(accepts(&selected.boundary(), &link));
    }

    #[test]
    fn shared_path_keeps_existence_policy() {
        let path = PathBuf::from("/site/shared");
        let optional = WatchRequirement::recursive_for(
            path.clone(),
            ProducerSet::one(ProducerKind::ConfiguredAssets),
        )
        .optional();
        let required =
            WatchRequirement::recursive_for(path.clone(), ProducerSet::one(ProducerKind::Content));

        let selection = selection_for(vec![optional, required]);

        assert_eq!(selection.desired.len(), 2);
        assert!(has_requirement(
            &selection.desired,
            &path,
            WatchScope::Recursive,
            ProducerKind::Content,
            ExistencePolicy::Required,
        ));
        assert!(has_requirement(
            &selection.desired,
            &path,
            WatchScope::Recursive,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Optional,
        ));
    }

    #[cfg(unix)]
    #[test]
    fn asset_evidence_tracks_retargeting() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let site = directory.path().canonicalize().unwrap().join("site");
        let external = directory.path().canonicalize().unwrap().join("external");
        let first_tree = external.join("tree-a");
        let second_tree = external.join("tree-b");
        let first_file = external.join("file-a.bin");
        let second_file = external.join("file-b.bin");
        std::fs::create_dir_all(site.join("content")).unwrap();
        std::fs::create_dir_all(&first_tree).unwrap();
        std::fs::create_dir_all(&second_tree).unwrap();
        std::fs::write(first_tree.join("member.bin"), b"first").unwrap();
        std::fs::write(second_tree.join("member.bin"), b"second").unwrap();
        std::fs::write(&first_file, b"first").unwrap();
        std::fs::write(&second_file, b"second").unwrap();
        let tree_link = site.join("tree-link");
        let file_link = site.join("file-link.bin");
        symlink(&first_tree, &tree_link).unwrap();
        symlink(&first_file, &file_link).unwrap();

        let schema = tola_build::config::SiteConfigSchema::parse(
            &site.join("tola.toml"),
            r#"[assets]
trees = [{ source = "tree-link", url-prefix = "/assets" }]
files = [{ source = "file-link.bin", url = "/download.bin" }]
"#,
            tola_build::InputScope::Online,
        )
        .unwrap();
        let config = resolve_schema(&site, schema, local_packages());
        std::fs::write(&config.build().entry, "#document(\"index.html\")[Site]").unwrap();
        let first =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Production)
                .unwrap();
        let first_tree_physical = std::fs::canonicalize(&first_tree).unwrap();
        let first_file_physical = std::fs::canonicalize(&first_file).unwrap();
        let dynamic = crate::dev::watch::requirements::watch_requirements_from_observation(
            &first.input_observation(),
        );
        let selection = selection_for(collect_roots(&config, &dynamic));

        assert!(has_requirement(
            &selection.desired,
            &tree_link,
            WatchScope::Recursive,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Required,
        ));
        assert!(has_requirement(
            &selection.desired,
            &file_link,
            WatchScope::Exact,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Required,
        ));
        assert!(has_requirement(
            &selection.desired,
            &first_tree_physical,
            WatchScope::Recursive,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Required,
        ));
        assert!(has_requirement(
            &selection.desired,
            &first_file_physical,
            WatchScope::Exact,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Required,
        ));
        assert!(
            selection
                .boundary()
                .classify(&first_tree_physical.join("member.bin"))
                .producers()
                .contains(ProducerKind::ConfiguredAssets)
        );
        assert!(
            selection
                .boundary()
                .classify(&first_file_physical)
                .producers()
                .contains(ProducerKind::ConfiguredAssets)
        );
        assert!(
            selection
                .boundary()
                .classify(&external.join("unowned.bin"))
                .is_empty(),
            "physical evidence must not expand to the external parent"
        );

        std::fs::remove_dir_all(&first_tree).unwrap();
        assert!(
            selection
                .boundary()
                .classify(&first_tree_physical)
                .producers()
                .contains(ProducerKind::ConfiguredAssets),
            "deletion of a published physical tree root must invalidate configured assets"
        );
        let missing_selection = selection_for(collect_roots(&config, &dynamic));
        assert!(has_requirement(
            &missing_selection.desired,
            &first_tree_physical,
            WatchScope::Recursive,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Required,
        ));

        std::fs::remove_file(&tree_link).unwrap();
        std::fs::remove_file(&file_link).unwrap();
        symlink(&second_tree, &tree_link).unwrap();
        symlink(&second_file, &file_link).unwrap();
        let retargeted =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Production)
                .unwrap();
        let retargeted_dynamic =
            crate::dev::watch::requirements::watch_requirements_from_observation(
                &retargeted.input_observation(),
            );
        let retargeted_selection = selection_for(collect_roots(&config, &retargeted_dynamic));
        assert!(has_requirement(
            &retargeted_selection.desired,
            &std::fs::canonicalize(&second_tree).unwrap(),
            WatchScope::Recursive,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Required,
        ));
        assert!(has_requirement(
            &retargeted_selection.desired,
            &std::fs::canonicalize(&second_file).unwrap(),
            WatchScope::Exact,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Required,
        ));
        assert!(
            retargeted_selection
                .boundary()
                .classify(&first_tree_physical.join("member.bin"))
                .is_empty()
        );
    }

    #[test]
    fn missing_file_watch_rejects_siblings() {
        let temp = TempDir::new().unwrap();
        let config = configured_site(
            temp.path(),
            "[assets]\ntrees = []\nfiles = [{ source = \"assets/CNAME\", url = \"/CNAME\" }]\n",
        );
        let root = config.get_root().to_path_buf();
        let content = root.join("content");
        let assets = root.join("assets");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&assets).unwrap();
        let target = assets.join("CNAME");

        let roots = collect_roots(&config, &WatchRequirements::default());
        let selection = selection_for(roots);
        assert!(has_requirement(
            &selection.desired,
            &target,
            WatchScope::Exact,
            ProducerKind::ConfiguredAssets,
            ExistencePolicy::Optional,
        ));
        assert!(watches(
            &selection,
            &assets.canonicalize().unwrap(),
            RecursiveMode::NonRecursive
        ));
        let boundary = selection.boundary();
        assert!(accepts(&boundary, &target));
        assert!(!accepts(&boundary, &assets.join("unrelated.txt")));
        assert!(!accepts(&boundary, &assets.join("other/deep.txt")));
    }

    #[test]
    fn hook_rerun_literals_are_watched() {
        use tola_build::config::section::build::hooks::{CommandOutput, OutputCommandConfig};

        let temp = TempDir::new().unwrap();
        let root = &temp.path().canonicalize().unwrap();
        let content = root.join("content");
        let output = root.join("public");
        let src = root.join(" scripts");
        let input = src.join("search.py");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(&input, "# search index generator").unwrap();

        let mut schema = tola_build::config::SiteConfigSchema::default();
        schema
            .build
            .hooks
            .generate_outputs
            .push(OutputCommandConfig {
                name: "search".into(),
                command: vec!["python3".into(), " scripts/search.py".into()],
                rerun_on: vec![" scripts/search.py".into()],
                outputs: vec![CommandOutput::File(
                    tola_address::OutputPath::parse("search.json").unwrap(),
                )],
                ..OutputCommandConfig::default()
            });

        let config = resolve_schema(root, schema, local_packages());
        let expected = input;
        let mut logical = Vec::new();
        super::collect_hook_rerun_paths(&config, &mut logical);
        assert!(has_requirement(
            &logical,
            &expected,
            WatchScope::Recursive,
            ProducerKind::Hooks,
            ExistencePolicy::Optional,
        ));

        let roots = collect_roots(&config, &WatchRequirements::default());
        let selected = selection_for(roots);
        assert!(
            selected
                .boundary()
                .classify(&expected)
                .producers()
                .contains(ProducerKind::Hooks)
        );
        assert!(selected.directories.iter().any(|directory| {
            directory.path == expected.parent().unwrap().canonicalize().unwrap()
                || (directory.mode == RecursiveMode::Recursive
                    && expected.starts_with(&directory.path))
        }));
    }

    #[test]
    fn recovery_watches_both_configs() {
        let (_directory, root, published) = default_site();
        let published_content = root.join("content");
        std::fs::write(
            &published.build().entry,
            "#document(\"index.html\")[Published]",
        )
        .unwrap();
        let mut attempted_schema = tola_build::config::SiteConfigSchema::default();
        let attempted_content = root.join("candidate/content");
        let attempted_entry = root.join("candidate/site.typ");
        attempted_schema.build.content_dir = PathBuf::from("candidate/content");
        attempted_schema.build.entry = PathBuf::from("candidate/site.typ");
        let attempted = resolve_schema(&root, attempted_schema, local_packages());
        let mut roots = RootSet::from_config(&published, &WatchRequirements::default());
        let initial = roots.initial_selection();
        install(&mut roots, initial, 1);
        let (recovery, _) =
            roots.configs_selection([&published, &attempted], &WatchRequirements::default());
        assert!(watches(&recovery, &root, RecursiveMode::NonRecursive));
        assert!(
            recovery
                .boundary()
                .classify(&attempted_content)
                .producers()
                .contains(ProducerKind::Content)
        );
        assert!(
            recovery
                .boundary()
                .classify(&attempted_entry)
                .producers()
                .contains(ProducerKind::TypstReads)
        );
        install(&mut roots, recovery, 2);
        std::fs::create_dir_all(&attempted_content).unwrap();
        std::fs::write(&attempted_entry, "#document(\"index.html\")[Recovered]").unwrap();
        let recovered = roots.current_selection();
        for content in [&published_content, &attempted_content] {
            assert!(watches(&recovered, content, RecursiveMode::Recursive));
        }
        assert!(accepts(&recovered.boundary(), &published.build().entry));
        assert!(accepts(&recovered.boundary(), &attempted_entry));
    }
}
