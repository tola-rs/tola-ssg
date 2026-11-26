use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_build::build::InputPathScope;
use tola_input::{FileSelection, lexical_path_identity};
use tola_module::hooks::RerunSelection;

use super::RebuildScope;

/// Declare every build producer once so its bit index, iteration order, and count stay aligned.
macro_rules! producers {
    ($($variant:ident),+ $(,)?) => {
        /// Build producer affected by a watched input; one input can invalidate several.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub(crate) enum ProducerKind {
            $($variant),+
        }

        pub(crate) const PRODUCER_KINDS: &[ProducerKind] = &[$(ProducerKind::$variant),+];

        const PRODUCER_COUNT: u32 = PRODUCER_KINDS.len() as u32;
    };
}

producers! {
    Content,
    TypstReads,
    Modules,
    SiteAssets,
    Fonts,
    Hooks,
    PackageResolution,
    Icons,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct ProducerSet(u16);

impl std::fmt::Debug for ProducerSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl ProducerSet {
    /// Force `PRODUCER_COUNT` into the mask width so exceeding 16 producers fails compilation.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum WatchScope {
    Exact,
    Children,
    Recursive,
}

/// Which names within one subscription can change its producer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ChangeAdmission {
    Scope,
    ContentExtension,
    AssetTree {
        selection: Arc<FileSelection>,
        prefix: PathBuf,
        member_depth: usize,
    },
    HookRerun {
        selection: Arc<RerunSelection>,
        logical_root: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct HookWatch {
    pub(crate) name: String,
    pub(crate) file: PathBuf,
    pub(crate) stage: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WatchRequirement {
    pub(super) path: PathBuf,
    pub(super) scope: WatchScope,
    pub(super) producers: ProducerSet,
    pub(super) required: bool,
    pub(super) admission: ChangeAdmission,
    pub(super) rebuild_scope: RebuildScope,
    pub(super) hook_watches: Vec<HookWatch>,
}

impl WatchRequirement {
    pub(super) fn for_scope(path: PathBuf, scope: WatchScope, producers: ProducerSet) -> Self {
        // Discovery reads content members only by `.typ` extension; exact scopes admit their named file.
        let admission = if scope != WatchScope::Exact && producers.contains(ProducerKind::Content) {
            ChangeAdmission::ContentExtension
        } else {
            ChangeAdmission::Scope
        };
        Self {
            path,
            scope,
            producers,
            required: true,
            admission,
            rebuild_scope: RebuildScope::Paths,
            hook_watches: Vec::new(),
        }
    }

    pub(super) fn recursive_for(path: PathBuf, producers: ProducerSet) -> Self {
        Self::for_scope(path, WatchScope::Recursive, producers)
    }

    pub(super) fn exact_for(path: PathBuf, producers: ProducerSet) -> Self {
        Self::for_scope(path, WatchScope::Exact, producers)
    }

    pub(super) fn with_required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub(super) fn optional(self) -> Self {
        self.with_required(false)
    }

    pub(super) fn with_rebuild_scope(mut self, rebuild_scope: RebuildScope) -> Self {
        self.rebuild_scope = rebuild_scope;
        self
    }

    pub(super) fn admits_event(&self, path: &Path, structural: bool) -> bool {
        if self.path.starts_with(path) && path != self.path {
            return structural
                || matches!(
                    self.admission,
                    ChangeAdmission::Scope | ChangeAdmission::ContentExtension
                );
        }
        if !self.matches(path) {
            return false;
        }
        match &self.admission {
            ChangeAdmission::Scope => true,
            ChangeAdmission::ContentExtension => {
                structural || self.path == path || tola_build::has_typ_extension(path)
            }
            ChangeAdmission::AssetTree {
                selection, prefix, ..
            } => path.strip_prefix(&self.path).is_ok_and(|rel| {
                let member;
                let candidate = if rel.as_os_str().is_empty() {
                    prefix.as_path()
                } else {
                    member = prefix.join(rel);
                    member.as_path()
                };
                selection.matches(candidate)
                    || (structural && selection.could_match_below(candidate))
            }),
            ChangeAdmission::HookRerun {
                selection,
                logical_root,
            } => path.strip_prefix(&self.path).is_ok_and(|rel| {
                let candidate = logical_root.join(rel);
                selection.matches(&candidate)
                    || (structural && selection.could_match_below(&candidate))
            }),
        }
    }

    pub(super) fn matches(&self, path: &Path) -> bool {
        match self.scope {
            WatchScope::Exact => path == self.path,
            WatchScope::Children => path == self.path || path.parent() == Some(&self.path),
            WatchScope::Recursive => path.starts_with(&self.path),
        }
    }

    pub(super) fn covers(&self, req: &Self) -> bool {
        let scope_covers = match self.scope {
            WatchScope::Recursive => req.path.starts_with(&self.path),
            WatchScope::Children => {
                (self.path == req.path && req.scope != WatchScope::Recursive)
                    || (req.scope == WatchScope::Exact && req.path.parent() == Some(&self.path))
            }
            WatchScope::Exact => self.path == req.path && req.scope == WatchScope::Exact,
        };
        scope_covers
            && (self.admission == ChangeAdmission::Scope
                || (self.admission == req.admission
                    && (self.admission == ChangeAdmission::ContentExtension
                        || self.path == req.path))
                || (req.scope == WatchScope::Exact && self.admits_event(&req.path, false)))
    }
}

/// Dynamic requirements discovered outside static site configuration.
#[derive(Debug, Clone, Default)]
pub(crate) struct WatchRequirements {
    requirements: Vec<WatchRequirement>,
    pub(super) hook_outputs: Vec<tola_input::FilesystemSourceIdentity>,
    pub(super) declared_hook_requirements: Vec<WatchRequirement>,
}

impl WatchRequirements {
    fn add(
        &mut self,
        scope: WatchScope,
        required: bool,
        rebuild_scope: RebuildScope,
        producer: ProducerKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.requirements.extend(paths.into_iter().map(|path| {
            WatchRequirement::for_scope(
                lexical_path_identity(&path),
                scope,
                ProducerSet::one(producer),
            )
            .with_required(required)
            .with_rebuild_scope(rebuild_scope)
        }));
    }

    /// Conservatively observe paths of hooks whose conditions are not evaluated yet.
    pub(crate) fn add_module_hook_inputs(&mut self, inputs: &tola_module::HookWatchInputs) {
        let start = self.requirements.len();
        self.requirements
            .extend(inputs.rerun_watches().iter().flat_map(|watch| {
                let source = HookWatch {
                    name: watch.name.clone(),
                    file: watch.manifest.clone(),
                    stage: watch.stage.as_str(),
                };
                hook_rerun_requirements(&watch.selection).map(move |mut req| {
                    req.hook_watches.push(source.clone());
                    req
                })
            }));
        self.observe_paths(
            ProducerKind::Hooks,
            InputPathScope::Exact,
            false,
            inputs.hook_scripts().iter().cloned(),
        );
        self.observe_paths(
            ProducerKind::Hooks,
            InputPathScope::Recursive,
            false,
            inputs.generated_paths().iter().cloned(),
        );
        self.declared_hook_requirements
            .extend_from_slice(&self.requirements[start..]);
        self.hook_outputs
            .extend(super::generated_source_identities(inputs));
    }

    pub(crate) fn observe_asset_tree(
        &mut self,
        root: &Path,
        required: bool,
        src_prefix: &Path,
        selection: &FileSelection,
    ) {
        let root = lexical_path_identity(root);
        let prefixes = selection
            .roots()
            .into_iter()
            .filter_map(|prefix| {
                let (member, prefix) = if let Ok(member) = prefix.strip_prefix(src_prefix) {
                    (member.to_path_buf(), prefix)
                } else if src_prefix.starts_with(&prefix) {
                    (PathBuf::new(), src_prefix.to_path_buf())
                } else {
                    return None;
                };
                (selection.matches(&prefix) || selection.could_match_below(&prefix))
                    .then_some((member, prefix))
            })
            .collect::<BTreeSet<_>>();
        let producers = ProducerSet::one(ProducerKind::SiteAssets);
        if !prefixes
            .iter()
            .any(|(member, _)| member.as_os_str().is_empty())
        {
            self.requirements
                .push(WatchRequirement::exact_for(root.clone(), producers).with_required(required));
        }
        let selection = Arc::new(selection.clone());
        self.requirements
            .extend(prefixes.into_iter().map(|(member, prefix)| {
                let mut req = WatchRequirement::recursive_for(root.join(&member), producers)
                    .with_required(required && member.as_os_str().is_empty());
                req.admission = ChangeAdmission::AssetTree {
                    selection: Arc::clone(&selection),
                    prefix,
                    member_depth: member.components().count(),
                };
                req
            }));
    }

    pub(crate) fn observe_paths(
        &mut self,
        producer: ProducerKind,
        scope: InputPathScope,
        required: bool,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.add(
            match scope {
                InputPathScope::Exact => WatchScope::Exact,
                InputPathScope::Children => WatchScope::Children,
                InputPathScope::Recursive => WatchScope::Recursive,
            },
            required,
            if producer == ProducerKind::Modules {
                RebuildScope::FullSite
            } else {
                RebuildScope::Paths
            },
            producer,
            paths,
        );
    }

    pub(crate) fn package_checks(
        &mut self,
        checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
    ) {
        self.requirements.extend(checks.into_iter().map(|check| {
            let required = matches!(
                check.availability(),
                tola_typst::PackageAvailability::Present
            );
            WatchRequirement::exact_for(
                lexical_path_identity(check.candidate()),
                ProducerSet::one(ProducerKind::PackageResolution),
            )
            .with_required(required)
            .with_rebuild_scope(RebuildScope::FullTypst)
        }));
    }

    pub(crate) fn package_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.add(
            WatchScope::Exact,
            false,
            RebuildScope::FullTypst,
            ProducerKind::PackageResolution,
            paths,
        );
    }

    pub(crate) fn typst_recovery(&mut self, path: PathBuf) {
        self.add(
            WatchScope::Recursive,
            false,
            RebuildScope::FullTypst,
            ProducerKind::TypstReads,
            [path],
        );
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &WatchRequirement> {
        self.requirements.iter()
    }
}

pub(super) fn hook_rerun_requirements(
    selection: &RerunSelection,
) -> impl Iterator<Item = WatchRequirement> {
    let roots = selection.roots();
    let selection = Arc::new(selection.clone());
    roots.into_iter().filter_map(move |path| {
        let path = lexical_path_identity(&path);
        if !selection.matches(&path) && !selection.could_match_below(&path) {
            return None;
        }
        let mut req =
            WatchRequirement::recursive_for(path.clone(), ProducerSet::one(ProducerKind::Hooks))
                .optional();
        req.admission = ChangeAdmission::HookRerun {
            selection: Arc::clone(&selection),
            logical_root: path,
        };
        Some(req)
    })
}

pub(super) fn dedupe_requirements(requirements: &mut Vec<WatchRequirement>) {
    requirements.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| right.required.cmp(&left.required))
            .then_with(|| left.rebuild_scope.cmp(&right.rebuild_scope))
    });
    let mut kept: Vec<WatchRequirement> = Vec::with_capacity(requirements.len());
    for req in requirements.drain(..) {
        let existing = kept
            .iter_mut()
            .rev()
            .take_while(|existing| {
                existing.path == req.path
                    && existing.scope == req.scope
                    && existing.required == req.required
            })
            .find(|existing| existing.admission == req.admission);
        if let Some(existing) = existing {
            existing.producers.insert(req.producers);
            existing.rebuild_scope = existing.rebuild_scope.max(req.rebuild_scope);
            existing.hook_watches.extend(req.hook_watches);
            existing.hook_watches.sort();
            existing.hook_watches.dedup();
        } else {
            kept.push(req);
        }
    }
    *requirements = kept;
}
