use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::RecursiveMode;

use super::boundary::{
    ClassifiedPath, CoverageCertainty, CoverageUncertainty, DirectoryWatch, DirectoryWatchUpdate,
    EventBoundary, WatchCoverage, WatchSelection, observation_contains,
};
use super::producer::{
    ChangeAdmission, HookWatch, ProducerKind, ProducerSet, WatchRequirement, WatchRequirements,
    WatchScope, dedupe_requirements, hook_rerun_requirements,
};
use tola_build::config::SiteConfig;
use tola_input::{lexical_path_identity, normalize_existing_prefix};

/// Current logical subscriptions and stable native directory coverage.
pub(super) struct RootSet {
    selection: WatchSelection,
    installed: Vec<DirectoryWatch>,
    coverage: BTreeMap<ProducerKind, WatchCoverage>,
}

impl RootSet {
    fn with_hook_outputs(
        desired: Vec<WatchRequirement>,
        hook_outputs: Arc<[tola_input::FilesystemSourceIdentity]>,
    ) -> Self {
        Self {
            selection: selection_for_observed(desired, &mut BTreeMap::new(), hook_outputs),
            installed: Vec::new(),
            coverage: BTreeMap::new(),
        }
    }

    pub(super) fn from_config(
        cfg: &SiteConfig,
        dynamic: &WatchRequirements,
    ) -> anyhow::Result<Self> {
        Ok(Self::with_hook_outputs(
            collect_roots(cfg, dynamic)?,
            combined_hook_outputs([cfg], dynamic),
        ))
    }

    pub(super) fn initial_selection(&self) -> WatchSelection {
        self.selection.clone()
    }

    /// Observe published and attempted configurations together during recovery.
    pub(super) fn configs_selection<'a>(
        &self,
        configs: impl IntoIterator<Item = &'a SiteConfig>,
        dynamic: &WatchRequirements,
    ) -> anyhow::Result<(WatchSelection, bool)> {
        let configs: Vec<&SiteConfig> = configs.into_iter().collect();
        let mut observed = BTreeMap::new();
        let outputs = combined_hook_outputs(configs.iter().copied(), dynamic);
        let mut configured_roots = Vec::new();
        for cfg in &configs {
            configured_roots.extend(collect_roots(cfg, &WatchRequirements::default())?);
        }
        configured_roots.extend(dynamic.declared_hook_requirements.iter().cloned());
        let mut candidate = configured_roots.clone();
        candidate.extend(dynamic.iter().cloned());
        let sel = selection_for_observed(candidate, &mut observed, Arc::clone(&outputs));
        let configured = selection_for_observed(configured_roots, &mut observed, outputs);
        Ok((sel, self.extends_observation(&configured)))
    }

    pub(super) fn staged_selection(
        &self,
        cfg: &SiteConfig,
        dynamic: &WatchRequirements,
    ) -> anyhow::Result<(WatchSelection, WatchSelection, bool)> {
        let mut configured_roots = collect_roots(cfg, &WatchRequirements::default())?;
        configured_roots.extend(dynamic.declared_hook_requirements.iter().cloned());
        let mut candidate = configured_roots.clone();
        candidate.extend(dynamic.iter().cloned());
        let mut desired = self.selection.desired.clone();
        desired.extend(candidate.iter().cloned());
        // Scoped to this stage: never reuse resolved symlinks across rounds.
        let mut observed = BTreeMap::new();
        let hook_outputs = combined_hook_outputs([cfg], dynamic);
        let observing = selection_for_observed(desired, &mut observed, Arc::clone(&hook_outputs));
        let subscriptions =
            selection_for_observed(candidate, &mut observed, Arc::clone(&hook_outputs));
        let configured = selection_for_observed(configured_roots, &mut observed, hook_outputs);
        // Only declared coverage retains refresh; discovered reads are reverified at handoff.
        Ok((
            observing,
            subscriptions,
            self.extends_observation(&configured),
        ))
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
        let mut dirs = boundary.directories.to_vec();
        dirs.extend(self.installed.iter().map(|dir| dir.path.clone()));
        dirs.sort();
        dirs.dedup();
        boundary.directories = dirs.into();
        boundary
    }

    pub(super) fn extends_observation(&self, sel: &WatchSelection) -> bool {
        !sel.directories.iter().all(|req| self.installed_covers(req))
            || sel.boundary.producers().iter().any(|producer| {
                !observation_contains(&self.selection.boundary, &sel.boundary, producer)
            })
    }

    fn installed_covers(&self, req: &DirectoryWatch) -> bool {
        self.installed.iter().any(|installed| installed.covers(req))
    }

    pub(super) fn has_installed_directories(&self) -> bool {
        !self.installed.is_empty()
    }

    /// Logical changes retain native watches; extension or repair installs the minimum set.
    pub(super) fn directory_update(
        &self,
        sel: &WatchSelection,
        refresh: bool,
    ) -> DirectoryWatchUpdate {
        if !refresh && sel.directories.iter().all(|req| self.installed_covers(req)) {
            return DirectoryWatchUpdate::Retain;
        }
        let directories = sel.directories.clone();
        let mut operations = Vec::new();
        for dir in &directories {
            if refresh || !self.installed.contains(dir) {
                operations.push(notify::PathOp::Watch(
                    dir.path().to_path_buf(),
                    notify::WatchPathConfig::new(dir.recursive_mode()),
                ));
            }
        }
        for installed in &self.installed {
            if !directories.iter().any(|dir| dir.path == installed.path) {
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
        sel: WatchSelection,
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
        self.replace_logical(sel, established_epoch, physical_changed);
    }

    /// Contract logical subscriptions without touching the backend; uncovered producers stay uncertain.
    pub(super) fn commit_logical(&mut self, sel: WatchSelection, established_epoch: u64) {
        self.replace_logical(sel, established_epoch, false);
    }

    fn replace_logical(
        &mut self,
        sel: WatchSelection,
        established_epoch: u64,
        physical_changed: bool,
    ) {
        let mut coverage = BTreeMap::new();
        for producer in sel.boundary.producers().iter() {
            let missing_coverage = sel.directories.iter().any(|req| {
                sel.directory_producers(req.path()).contains(producer)
                    && !self.installed_covers(req)
            });
            let certainty = if missing_coverage {
                CoverageCertainty::Uncertain(CoverageUncertainty::CoverageGap)
            } else {
                sel.uncertainties
                    .get(&producer)
                    .copied()
                    .map(CoverageCertainty::Uncertain)
                    .unwrap_or(CoverageCertainty::AcceptedEvents)
            };
            let prev = self.coverage.get(&producer);
            let continuous = !physical_changed
                && observation_contains(&self.selection.boundary, &sel.boundary, producer)
                && prev.is_some_and(|prev| prev.certainty == certainty);
            let observation = if continuous {
                prev.expect("continuous coverage has a previous observation")
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
        self.selection = sel;
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

struct WatchPathObservation {
    physical: Option<PathBuf>,
    entries: Vec<(PathBuf, Option<PathBuf>)>,
    parent: Option<PathBuf>,
    is_dir: Result<bool, std::io::ErrorKind>,
    unreadable: bool,
}

impl WatchPathObservation {
    fn capture(path: &Path, tree_prefix_depth: usize) -> Self {
        let resolved = resolve_symlinks(path, tree_prefix_depth);
        let entries = resolved
            .entries
            .into_iter()
            .map(|entry| {
                let entry = entry
                    .parent()
                    .zip(entry.file_name())
                    .map(|(parent, name)| normalize_existing_prefix(parent).join(name))
                    .unwrap_or(entry);
                let parent = entry.parent().and_then(nearest_existing_dir);
                (entry, parent)
            })
            .collect();
        if resolved.symlinked_member {
            return Self {
                physical: None,
                entries,
                parent: None,
                is_dir: Ok(false),
                unreadable: false,
            };
        }
        let physical = normalize_existing_prefix(&resolved.physical);
        let parent = physical.parent().and_then(nearest_existing_dir);
        let is_dir = std::fs::metadata(&physical)
            .map(|meta| meta.is_dir())
            .map_err(|err| err.kind());
        Self {
            physical: Some(physical),
            entries,
            parent,
            is_dir,
            unreadable: resolved.unreadable,
        }
    }
}

fn selection_for_observed(
    mut desired: Vec<WatchRequirement>,
    observed: &mut BTreeMap<(PathBuf, usize), WatchPathObservation>,
    hook_outputs: Arc<[tola_input::FilesystemSourceIdentity]>,
) -> WatchSelection {
    dedupe_requirements(&mut desired);
    let mut matched = Vec::new();
    let mut dirs = Vec::new();
    let mut dir_producers = BTreeMap::<PathBuf, ProducerSet>::new();
    let mut symlink_entries = Vec::new();
    let mut uncertainties = BTreeMap::new();

    for req in &desired {
        let tree_prefix_depth = match &req.admission {
            ChangeAdmission::AssetTree { member_depth, .. } => *member_depth,
            _ => 0,
        };
        let resolved = observed
            .entry((req.path.clone(), tree_prefix_depth))
            .or_insert_with(|| WatchPathObservation::capture(&req.path, tree_prefix_depth));
        if resolved.unreadable {
            record_uncertainty(
                &mut uncertainties,
                req.producers,
                CoverageUncertainty::Unreadable,
            );
        }
        for (entry, parent) in &resolved.entries {
            symlink_entries.push(entry.clone());
            let mut entry_req = WatchRequirement::exact_for(entry.clone(), req.producers)
                .with_rebuild_scope(req.rebuild_scope)
                .optional();
            entry_req.hook_watches = req.hook_watches.clone();
            matched.push(entry_req);
            record_parent_watch(
                &mut dirs,
                &mut dir_producers,
                &mut uncertainties,
                parent.as_ref(),
                req.producers,
            );
        }
        let Some(physical_path) = &resolved.physical else {
            continue;
        };
        matched.push(req.clone());
        if *physical_path != req.path {
            let mut physical = req.clone();
            physical.path = physical_path.clone();
            matched.push(physical);
        }
        match resolved.is_dir {
            Ok(true) if req.scope != WatchScope::Exact => {
                let mode = if req.scope == WatchScope::Recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                add_directory(
                    &mut dirs,
                    &mut dir_producers,
                    physical_path.clone(),
                    mode,
                    req.producers,
                );
                if let Some(parent) = &resolved.parent {
                    add_directory(
                        &mut dirs,
                        &mut dir_producers,
                        parent.clone(),
                        RecursiveMode::NonRecursive,
                        req.producers,
                    );
                }
            }
            meta => {
                match meta {
                    Err(std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
                        if req.required =>
                    {
                        record_uncertainty(
                            &mut uncertainties,
                            req.producers,
                            CoverageUncertainty::MissingRequired,
                        );
                    }
                    Err(std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
                    | Ok(_) => {}
                    Err(_) => record_uncertainty(
                        &mut uncertainties,
                        req.producers,
                        CoverageUncertainty::Unreadable,
                    ),
                }
                record_parent_watch(
                    &mut dirs,
                    &mut dir_producers,
                    &mut uncertainties,
                    resolved.parent.as_ref(),
                    req.producers,
                );
            }
        }
    }

    minimize_directories(&mut dirs);
    let directory_producers = dirs
        .iter()
        .map(|dir| {
            let producers = if dir.mode == RecursiveMode::Recursive {
                dir_producers
                    .range(dir.path.clone()..)
                    .take_while(|(path, _)| path.starts_with(&dir.path))
                    .fold(ProducerSet::default(), |mut acc, (_, owners)| {
                        acc.insert(*owners);
                        acc
                    })
            } else {
                dir_producers.get(&dir.path).copied().unwrap_or_default()
            };
            (dir.path.clone(), producers)
        })
        .collect();
    dedupe_requirements(&mut matched);
    symlink_entries.sort();
    symlink_entries.dedup();
    let producers = matched.iter().fold(ProducerSet::default(), |mut acc, req| {
        acc.insert(req.producers);
        acc
    });
    let boundary = EventBoundary {
        producers,
        desired: matched.into(),
        directories: dirs.iter().map(|dir| dir.path.clone()).collect(),
        symlink_entries: symlink_entries.into(),
        hook_outputs,
    };
    WatchSelection {
        desired,
        boundary,
        directories: dirs,
        directory_producers,
        uncertainties,
    }
}

fn record_parent_watch(
    dirs: &mut Vec<DirectoryWatch>,
    dir_producers: &mut BTreeMap<PathBuf, ProducerSet>,
    uncertainties: &mut BTreeMap<ProducerKind, CoverageUncertainty>,
    parent: Option<&PathBuf>,
    producers: ProducerSet,
) {
    match parent {
        Some(parent) => add_directory(
            dirs,
            dir_producers,
            parent.clone(),
            RecursiveMode::NonRecursive,
            producers,
        ),
        None => record_uncertainty(uncertainties, producers, CoverageUncertainty::CoverageGap),
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
    dirs: &mut Vec<DirectoryWatch>,
    dir_producers: &mut BTreeMap<PathBuf, ProducerSet>,
    path: PathBuf,
    mode: RecursiveMode,
    producers: ProducerSet,
) {
    dir_producers
        .entry(path.clone())
        .or_default()
        .insert(producers);
    dirs.push(DirectoryWatch { path, mode });
}

fn minimize_directories(dirs: &mut Vec<DirectoryWatch>) {
    let mut modes = BTreeMap::new();
    for dir in dirs.drain(..) {
        modes
            .entry(dir.path)
            .and_modify(|mode| {
                if dir.mode == RecursiveMode::Recursive {
                    *mode = RecursiveMode::Recursive;
                }
            })
            .or_insert(dir.mode);
    }
    let mut minimal = Vec::new();
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
    *dirs = minimal;
}

struct PhysicalWatchPath {
    physical: PathBuf,
    entries: Vec<PathBuf>,
    unreadable: bool,
    symlinked_member: bool,
}

/// Keep link entries for retargeting and resolved paths for target creation.
fn resolve_symlinks(path: &Path, tree_prefix_depth: usize) -> PhysicalWatchPath {
    let mut physical = lexical_path_identity(path);
    let mut entries = BTreeSet::new();
    let finish =
        |physical, entries: BTreeSet<PathBuf>, unreadable, symlinked_member| PhysicalWatchPath {
            physical,
            entries: entries.into_iter().collect(),
            unreadable,
            symlinked_member,
        };
    loop {
        let mut prefix = PathBuf::new();
        let components = physical.components().collect::<Vec<_>>();
        let mut replacement = None;
        for (idx, component) in components.iter().enumerate() {
            prefix.push(component.as_os_str());
            if matches!(
                component,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            ) {
                continue;
            }
            match std::fs::symlink_metadata(&prefix) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    let prefix = lexical_path_identity(&prefix);
                    if idx >= components.len() - tree_prefix_depth {
                        entries.insert(prefix);
                        return finish(physical, entries, false, true);
                    }
                    if !entries.insert(prefix.clone()) {
                        return finish(physical, entries, true, false);
                    }
                    let Ok(target) = std::fs::read_link(&prefix) else {
                        return finish(physical, entries, true, false);
                    };
                    let mut rewritten = if target.is_absolute() {
                        target
                    } else {
                        prefix.parent().unwrap_or(Path::new("")).join(target)
                    };
                    for suffix in &components[idx + 1..] {
                        rewritten.push(suffix.as_os_str());
                    }
                    replacement = Some(rewritten);
                    break;
                }
                Ok(_) => {}
                Err(err) => {
                    let unreadable = !matches!(
                        err.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    );
                    return finish(physical, entries, unreadable, false);
                }
            }
        }
        match replacement {
            Some(rewritten) => physical = rewritten,
            None => return finish(physical, entries, false, false),
        }
    }
}

fn nearest_existing_dir(path: &Path) -> Option<PathBuf> {
    path.ancestors().find_map(|ancestor| {
        std::fs::metadata(ancestor)
            .ok()
            .filter(|meta| meta.is_dir())
            .and_then(|_| ancestor.canonicalize().ok())
    })
}

fn collect_roots(
    cfg: &SiteConfig,
    dynamic: &WatchRequirements,
) -> anyhow::Result<Vec<WatchRequirement>> {
    let content_producers =
        ProducerSet::one(ProducerKind::Content) | ProducerSet::one(ProducerKind::TypstReads);
    let mut roots = vec![WatchRequirement::recursive_for(
        lexical_path_identity(&cfg.build().content_dir),
        content_producers,
    )];
    if let Some(entry) = cfg.project_entry_path() {
        roots.push(WatchRequirement::exact_for(
            lexical_path_identity(&entry),
            ProducerSet::one(ProducerKind::TypstReads),
        ));
    }
    roots.extend(cfg.fonts().paths.iter().map(|path| {
        WatchRequirement::recursive_for(
            lexical_path_identity(path),
            ProducerSet::one(ProducerKind::Fonts),
        )
        .optional()
    }));

    let mut configured_reqs = WatchRequirements::default();
    for tree in &cfg.assets().trees {
        configured_reqs.observe_asset_tree(
            tree.source(),
            false,
            &cfg.root_relative(tree.source()),
            tree.selection(),
        );
    }
    for src in cfg.assets().file_sources() {
        roots.push(
            WatchRequirement::exact_for(
                lexical_path_identity(src),
                ProducerSet::one(ProducerKind::SiteAssets),
            )
            .optional(),
        );
    }

    let configured = tola_build::build::configured_input_observation(cfg);
    crate::dev::watch::requirements::append_observed_paths(
        &mut configured_reqs,
        configured.paths(),
    );
    roots.extend(configured_reqs.iter().cloned());

    collect_hook_requirements(cfg, &mut roots)?;
    roots.extend(dynamic.iter().cloned());

    for path in cfg.config_paths() {
        roots.push(
            WatchRequirement::exact_for(lexical_path_identity(path), ProducerSet::ALL).optional(),
        );
    }

    dedupe_requirements(&mut roots);
    Ok(roots)
}

fn site_relative_path(root: &Path, declared: &Path) -> PathBuf {
    if declared.is_absolute() {
        lexical_path_identity(declared)
    } else {
        lexical_path_identity(&root.join(declared))
    }
}

pub(super) fn development_before_build_hooks(
    cfg: &SiteConfig,
) -> impl Iterator<Item = &tola_build::config::section::build::BeforeBuildHookConfig> {
    cfg.hooks()
        .before_build
        .iter()
        .filter(|hook| hook.enable && hook.dev.participates_in_development())
}

fn declared_hook_outputs(
    cfg: &SiteConfig,
) -> impl Iterator<Item = tola_input::FilesystemSourceIdentity> + '_ {
    development_before_build_hooks(cfg)
        .flat_map(|hook| hook.generates.iter())
        .map(move |output| {
            tola_input::FilesystemSourceIdentity::from_path(&site_relative_path(cfg.root(), output))
        })
}

fn combined_hook_outputs<'a>(
    configs: impl IntoIterator<Item = &'a SiteConfig>,
    dynamic: &WatchRequirements,
) -> Arc<[tola_input::FilesystemSourceIdentity]> {
    let configs: Vec<_> = configs.into_iter().collect();
    let mut outputs: Vec<_> = configs
        .iter()
        .flat_map(|cfg| declared_hook_outputs(cfg))
        .collect();
    outputs.sort_by(|left, right| left.logical_path().cmp(right.logical_path()));
    outputs.dedup_by(|left, right| left.logical_path() == right.logical_path());
    outputs.extend(dynamic.hook_outputs.iter().cloned());
    outputs.sort();
    outputs.dedup();
    outputs.into()
}

fn collect_hook_requirements(
    cfg: &SiteConfig,
    roots: &mut Vec<WatchRequirement>,
) -> anyhow::Result<()> {
    for hook in development_before_build_hooks(cfg) {
        for output in &hook.generates {
            let logical = site_relative_path(cfg.root(), output);
            let identity = tola_input::FilesystemSourceIdentity::from_path(&logical);
            let scope = match std::fs::metadata(&logical) {
                Ok(meta) if meta.is_file() => WatchScope::Exact,
                _ => WatchScope::Recursive,
            };
            roots.extend(
                [
                    identity.logical_path().to_path_buf(),
                    identity.physical_path().to_path_buf(),
                ]
                .into_iter()
                .map(|path| {
                    WatchRequirement::for_scope(path, scope, ProducerSet::one(ProducerKind::Hooks))
                        .optional()
                }),
            );
        }
    }
    macro_rules! stage {
        ($field:ident, $name:literal) => {
            for hook in &cfg.hooks().$field {
                if !hook.enable
                    || !hook.dev.participates_in_development()
                    || hook.rerun_on.include.is_empty()
                {
                    continue;
                }
                let sel = hook.rerun_on.compile(cfg.root(), hook.origin.as_ref())?;
                let watch = HookWatch {
                    name: hook.declared_name.as_ref().unwrap_or(&hook.name).clone(),
                    file: hook.origin.as_ref().map_or_else(
                        || cfg.config_path().to_owned(),
                        |origin| origin.manifest.clone(),
                    ),
                    stage: $name,
                };
                roots.extend(hook_rerun_requirements(&sel).map(|mut req| {
                    req.hook_watches.push(watch.clone());
                    req
                }));
            }
        };
    }
    stage!(before_build, "before-build");
    stage!(generate_outputs, "generate-outputs");
    stage!(after_publish, "after-publish");
    roots.extend(
        cfg.hooks()
            .development_hook_scripts(cfg.root())
            .map(|path| {
                WatchRequirement::exact_for(
                    lexical_path_identity(&path),
                    ProducerSet::one(ProducerKind::Hooks),
                )
                .optional()
            }),
    );
    Ok(())
}

#[cfg(test)]
#[path = "roots_tests.rs"]
mod tests;
