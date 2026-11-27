//! Module hook declarations and the build hooks selected for each instance.

use crate::options::OptionPath;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use tola_input::{FilePatterns, FileSelection};

use crate::bundle::{HookEntry, HookParticipation, HooksContribution};
use crate::identity::ModuleId;
use crate::module_set::{LoadedModule, ModuleOwner};
use crate::watch::HookWatchInputs;

/// Hooks grouped by the event that runs them.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct ModuleHooks {
    before_build: Vec<BeforeBuild>,
    generate_outputs: Vec<GenerateOutputs>,
    after_publish: Vec<AfterPublish>,
    pub before_pack: Vec<BeforePackHook>,
}

macro_rules! build_hook {
    ($type:ident, $stage:ident $(, $($field:tt)*)?) => {
        #[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        struct $type {
            name: String,
            #[serde(default)]
            order: u64,
            #[serde(default = "enabled")]
            enable: bool,
            nu: Option<PathBuf>,
            argv: Option<Vec<String>>,
            args: Option<Vec<String>>,
            enabled_by: Option<OptionPath>,
            dev: Option<DevParticipation>,
            #[serde(default)]
            rerun_on: RerunOn,
            $($($field)*)?
        }

        impl $type {
            fn hook(&self) -> Hook<'_> {
                Hook {
                    stage: HookStage::$stage,
                    name: &self.name,
                    order: self.order,
                    enable: self.enable,
                    nu: &self.nu,
                    argv: &self.argv,
                    args: &self.args,
                    enabled_by: self.enabled_by.as_ref(),
                    dev: self.dev.unwrap_or(match HookStage::$stage {
                        HookStage::AfterPublish => DevParticipation::Skip,
                        _ => DevParticipation::Run,
                    }),
                    rerun_on: &self.rerun_on,
                    generates: &[],
                    outputs: &[],
                }
            }
        }
    }
}

build_hook!(BeforeBuild, BeforeBuild, #[serde(default)] generates: Vec<PathBuf>);
build_hook!(GenerateOutputs, GenerateOutputs, outputs: Vec<CommandOutput>);
build_hook!(AfterPublish, AfterPublish);

/// An author action run before pack enumerates the module's delivery files.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BeforePackHook {
    pub name: String,
    #[serde(default = "enabled")]
    pub enable: bool,
    nu: Option<PathBuf>,
    argv: Option<Vec<String>>,
    args: Option<Vec<String>>,
}

impl BeforePackHook {
    /// Executable action of this author hook.
    pub fn action(&self) -> std::result::Result<crate::Action, String> {
        crate::Action::from_fields(self.nu.clone(), self.argv.clone(), self.args.clone())
    }
}

fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy)]
struct Hook<'a> {
    stage: HookStage,
    name: &'a str,
    order: u64,
    enable: bool,
    nu: &'a Option<PathBuf>,
    argv: &'a Option<Vec<String>>,
    args: &'a Option<Vec<String>>,
    enabled_by: Option<&'a OptionPath>,
    dev: DevParticipation,
    rerun_on: &'a RerunOn,
    generates: &'a [PathBuf],
    outputs: &'a [CommandOutput],
}

impl<'a> Hook<'a> {
    fn action(self) -> std::result::Result<crate::Action, String> {
        crate::Action::from_fields(self.nu.clone(), self.argv.clone(), self.args.clone())
    }
}

impl ModuleHooks {
    pub(crate) fn runtime_scripts(&self) -> impl Iterator<Item = (HookStage, &str, &Path)> {
        self.build_hooks()
            .filter_map(|hook| Some((hook.stage, hook.name, hook.nu.as_deref()?)))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.before_build.is_empty()
            && self.generate_outputs.is_empty()
            && self.after_publish.is_empty()
            && self.before_pack.is_empty()
    }

    fn build_hooks(&self) -> impl Iterator<Item = Hook<'_>> {
        self.before_build
            .iter()
            .map(|hook| Hook {
                generates: &hook.generates,
                ..hook.hook()
            })
            .chain(self.generate_outputs.iter().map(|hook| Hook {
                outputs: &hook.outputs,
                ..hook.hook()
            }))
            .chain(self.after_publish.iter().map(AfterPublish::hook))
    }
}

/// Extra hook inputs, selected by paths or globs.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RerunOn {
    pub include: Vec<RerunPath>,
    #[serde(default)]
    pub exclude: Vec<RerunPath>,
}

/// One path or glob relative to its declaring root, its owner, or explicitly to the project.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum RerunPath {
    Relative(String),
    Project { project: String },
    Owner { owner: String },
}

fn has_forbidden_marker(text: &str) -> bool {
    ["{module}", "{project}", "{tola}"]
        .iter()
        .any(|marker| text.contains(marker))
}

impl RerunPath {
    fn pattern(&self) -> &str {
        match self {
            Self::Relative(pat) | Self::Project { project: pat } | Self::Owner { owner: pat } => {
                pat
            }
        }
    }

    fn validate(&self, module: bool, include: bool) -> Result<()> {
        let raw = self.pattern();
        let pat = if module {
            ensure!(
                !has_forbidden_marker(raw),
                "the pattern `{raw}` only accepts the `{{name}}` substitution"
            );
            raw.replace("{name}", "module")
        } else {
            raw.to_owned()
        };
        let mut pats = FilePatterns::default();
        if include {
            pats.include.push(pat);
        } else {
            pats.exclude.push(pat);
        }
        FileSelection::new(&pats).map_err(|err| err.at("rerun-on"))?;
        Ok(())
    }
}

impl RerunOn {
    /// Validate each pattern before the site or module root is known.
    pub fn validate(
        &self,
        module: bool,
    ) -> impl Iterator<Item = (&'static str, usize, anyhow::Error)> + '_ {
        [(true, &self.include), (false, &self.exclude)]
            .into_iter()
            .flat_map(move |(include, pats)| {
                let key = if include { "include" } else { "exclude" };
                pats.iter().enumerate().filter_map(move |(idx, pat)| {
                    pat.validate(module, include)
                        .err()
                        .map(|err| (key, idx, err))
                })
            })
    }

    fn expand(&self, alias: &str) -> Self {
        let expand = |entry: &RerunPath| match entry {
            RerunPath::Relative(pat) => RerunPath::Relative(pat.replace("{name}", alias)),
            RerunPath::Project { project } => RerunPath::Project {
                project: project.replace("{name}", alias),
            },
            RerunPath::Owner { owner } => RerunPath::Owner {
                owner: owner.replace("{name}", alias),
            },
        };
        Self {
            include: self.include.iter().map(expand).collect(),
            exclude: self.exclude.iter().map(expand).collect(),
        }
    }

    /// Compile the hook's extra inputs for watching or fingerprinting.
    ///
    /// Module patterns must already have their instance name substituted.
    pub fn compile(
        &self,
        site_root: &Path,
        origin: Option<&ModuleHookOrigin>,
    ) -> Result<RerunSelection> {
        let mod_origin = origin.map(|o| (o.name.as_str(), o.root.as_path()));
        let owner = origin.map(|o| &o.owner);
        self.compile_at(site_root, mod_origin, owner)
    }

    fn compile_at(
        &self,
        site_root: &Path,
        module: Option<(&str, &Path)>,
        owner: Option<&ModuleOwner>,
    ) -> Result<RerunSelection> {
        let mut site_pats = FilePatterns::default();
        let mut mod_pats = FilePatterns::default();
        let mut owner_pats = FilePatterns::default();
        let owner_module = owner.and_then(|o| match o {
            ModuleOwner::Site => None,
            ModuleOwner::Module { id, root } => Some((id.as_str(), root.as_path())),
        });
        for (pats, site, local, owned) in [
            (
                &self.include,
                &mut site_pats.include,
                &mut mod_pats.include,
                &mut owner_pats.include,
            ),
            (
                &self.exclude,
                &mut site_pats.exclude,
                &mut mod_pats.exclude,
                &mut owner_pats.exclude,
            ),
        ] {
            for pat in pats {
                match pat {
                    RerunPath::Relative(p) if module.is_some() => local.push(p.clone()),
                    RerunPath::Owner { owner: p } if owner_module.is_some() => {
                        owned.push(p.clone())
                    }
                    _ => site.push(pat.pattern().to_owned()),
                }
            }
        }
        let mut scopes = Vec::new();
        compile_scope(site_root, site_pats, None, &mut scopes)?;
        if let Some((alias, root)) = module {
            compile_scope(root, mod_pats, Some(alias), &mut scopes)?;
        }
        if let Some((owner_alias, owner_root)) = owner_module {
            compile_scope(owner_root, owner_pats, Some(owner_alias), &mut scopes)?;
        }
        Ok(RerunSelection { scopes })
    }
}

fn compile_scope(
    root: &Path,
    pats: FilePatterns,
    alias: Option<&str>,
    scopes: &mut Vec<(PathBuf, FileSelection)>,
) -> Result<()> {
    if pats.include.is_empty() {
        if !pats.exclude.is_empty() {
            scopes.push((root.to_owned(), FileSelection::new(&pats)?));
        }
        return Ok(());
    }
    // Each include decides independently whether it names `.git`/`.jj` so a broad
    // pattern cannot admit unrelated VCS internals.
    for include in pats.include {
        let sel = FileSelection::new(&FilePatterns {
            include: vec![include],
            exclude: pats.exclude.clone(),
        })?;
        if let Some(alias) = alias {
            for path in sel.roots() {
                ensure!(
                    tola_input::normalize_existing_prefix(&root.join(&path)).starts_with(root),
                    "the `rerun-on` path `{}` leaves module `{alias}`",
                    path.display()
                );
            }
        }
        scopes.push((root.to_owned(), sel));
    }
    Ok(())
}

/// Compiled hook input rules paired with their absolute roots.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RerunSelection {
    scopes: Vec<(PathBuf, FileSelection)>,
}

impl RerunSelection {
    /// Whether an absolute path matches an include before exclusions.
    pub fn includes(&self, path: &Path) -> bool {
        self.scopes.iter().any(|(root, selection)| {
            path.strip_prefix(root)
                .is_ok_and(|path| admits_vcs_path(path, selection) && selection.includes(path))
        })
    }

    /// Whether an absolute path matches an exclusion in either scope.
    pub fn excludes(&self, path: &Path) -> bool {
        self.scopes.iter().any(|(root, selection)| {
            path.strip_prefix(root)
                .is_ok_and(|path| selection.excludes(path))
        })
    }

    /// Whether an absolute path is selected as an input.
    pub fn matches(&self, path: &Path) -> bool {
        self.includes(path) && !self.excludes(path)
    }

    /// Whether an absolute directory can contain selected inputs.
    pub fn could_match_below(&self, path: &Path) -> bool {
        !self.excludes(path)
            && self.scopes.iter().any(|(root, selection)| {
                if let Ok(path) = path.strip_prefix(root) {
                    admits_vcs_path(path, selection) && selection.could_match_below(path)
                } else {
                    root.starts_with(path) && selection.could_match_below(Path::new(""))
                }
            })
    }

    /// Fixed prefixes where selected inputs can be found.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.scoped_roots().map(|(_, path)| path).collect()
    }

    /// Each fixed prefix paired with the absolute scope where its pattern starts.
    pub fn scoped_roots(&self) -> impl Iterator<Item = (&Path, PathBuf)> + '_ {
        self.scopes.iter().flat_map(|(root, selection)| {
            selection
                .roots()
                .into_iter()
                .map(move |path| (root.as_path(), root.join(path)))
        })
    }

    /// Literal includes, including paths not yet present on disk.
    pub fn literal_includes(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.scopes.iter().flat_map(|(root, selection)| {
            selection
                .literal_includes()
                .map(move |path| root.join(path))
        })
    }

    /// Whether a literal input's physical target stays within its declared scope.
    pub fn admits_literal_target(&self, literal: &Path, target: &Path) -> bool {
        !self.excludes(target)
            && self.scopes.iter().any(|(root, selection)| {
                literal.strip_prefix(root).is_ok_and(|literal| {
                    selection.literal_includes().any(|path| path == literal)
                        && target
                            .strip_prefix(root)
                            .is_ok_and(|target| admits_vcs_path(target, selection))
                })
            })
    }
}

/// Metadata is an input only when this include names its `.git` or `.jj` component.
fn admits_vcs_path(path: &Path, selection: &FileSelection) -> bool {
    let directory = path
        .components()
        .find(|part| part.as_os_str() == ".git" || part.as_os_str() == ".jj");
    directory.is_none_or(|directory| {
        selection.patterns().include.iter().any(|pattern| {
            Path::new(pattern)
                .components()
                .any(|part| part == directory)
        })
    })
}

/// A hook declaration a module manifest cannot use.
#[derive(Debug, thiserror::Error)]
pub enum HookDeclarationError {
    #[error("a hook name must be one word")]
    Name,
    #[error("hook `{name}` is declared twice in one stage")]
    DuplicateName { name: String },
    #[error("hook `{hook}`: {reason}")]
    Action { hook: String, reason: String },
    #[error("hook `{hook}` in `after-publish` does not accept `dev = \"reuse\"`")]
    AfterPublishReuse { hook: String },
    #[error(
        "hook `{hook}` must declare files or directories in `generates` to use `dev = \"reuse\"`"
    )]
    ReuseNeedsGenerates { hook: String },
    #[error("`{label}` only accepts the `{{name}}` substitution")]
    Substitution { label: &'static str },
    #[error("`{label}` must be a relative path inside its module or the site")]
    Path { label: &'static str },
    #[error("hook `{hook}` has an invalid `rerun-on.{key}` entry: {reason}")]
    RerunPattern {
        hook: String,
        key: &'static str,
        reason: String,
    },
    #[error("a `generate-outputs` hook declares no outputs")]
    GenerateOutputsNoOutputs,
    #[error("an output only accepts the `{{name}}` substitution")]
    OutputSubstitution,
    #[error("`{path}` is reserved for Tola")]
    OutputReserved { path: String },
    #[error("the hook declares `{path}` twice")]
    OutputRepeated { path: String },
}

fn relative(path: &Path, label: &'static str) -> std::result::Result<(), HookDeclarationError> {
    if has_forbidden_marker(&path.to_string_lossy()) {
        return Err(HookDeclarationError::Substitution { label });
    }
    if !crate::declaration::is_nonempty_relative_path(path) {
        return Err(HookDeclarationError::Path { label });
    }
    Ok(())
}

fn expand_path(path: &Path, alias: &str) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace("{name}", alias))
}

fn expand_outputs(outputs: &[CommandOutput], alias: &str) -> Result<Vec<CommandOutput>> {
    let mut seen = BTreeSet::new();
    outputs
        .iter()
        .map(|out| {
            let path =
                tola_address::OutputPath::parse(&out.path().as_str().replace("{name}", alias))?;
            ensure!(
                !path.is_reserved_for_non_system_output(),
                "`{path}` is reserved for Tola"
            );
            ensure!(
                seen.insert(path.clone()),
                "the hook generates `{path}` twice after `{{name}}` substitution"
            );
            Ok(match out {
                CommandOutput::File(_) => CommandOutput::File(path),
                CommandOutput::Tree(_) => CommandOutput::Tree(path),
            })
        })
        .collect()
}

fn validate_hook_name<'a>(
    names: &mut BTreeSet<(Option<HookStage>, &'a str)>,
    stage: Option<HookStage>,
    name: &'a str,
) -> std::result::Result<(), HookDeclarationError> {
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(HookDeclarationError::Name);
    }
    if !names.insert((stage, name)) {
        return Err(HookDeclarationError::DuplicateName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn validate_build_hook(hook: Hook<'_>) -> std::result::Result<(), HookDeclarationError> {
    hook.action()
        .map_err(|reason| HookDeclarationError::Action {
            hook: hook.name.into(),
            reason,
        })?;
    if hook.dev.reuses_outputs() {
        match hook.stage {
            HookStage::AfterPublish => {
                return Err(HookDeclarationError::AfterPublishReuse {
                    hook: hook.name.into(),
                });
            }
            HookStage::BeforeBuild if hook.generates.is_empty() => {
                return Err(HookDeclarationError::ReuseNeedsGenerates {
                    hook: hook.name.into(),
                });
            }
            _ => {}
        }
    }
    if let Some((key, _, err)) = hook.rerun_on.validate(true).next() {
        return Err(HookDeclarationError::RerunPattern {
            hook: hook.name.to_owned(),
            key,
            reason: err.to_string(),
        });
    }
    for path in hook.generates {
        relative(path, "generates")?;
    }
    if hook.stage == HookStage::GenerateOutputs && hook.outputs.is_empty() {
        return Err(HookDeclarationError::GenerateOutputsNoOutputs);
    }
    let mut seen = BTreeSet::new();
    for out in hook.outputs {
        let path = out.path().as_str();
        if has_forbidden_marker(path) {
            return Err(HookDeclarationError::OutputSubstitution);
        }
        if out.path().is_reserved_for_non_system_output() {
            return Err(HookDeclarationError::OutputReserved { path: path.into() });
        }
        if !seen.insert(out.path()) {
            return Err(HookDeclarationError::OutputRepeated { path: path.into() });
        }
    }
    Ok(())
}

/// Validate all declarations, including those disabled by options.
pub fn validate_declarations(hooks: &ModuleHooks) -> std::result::Result<(), HookDeclarationError> {
    let mut names = BTreeSet::new();
    for hook in hooks.build_hooks() {
        validate_hook_name(&mut names, Some(hook.stage), hook.name)?;
        validate_build_hook(hook)?;
    }
    for hook in &hooks.before_pack {
        validate_hook_name(&mut names, None, &hook.name)?;
        hook.action()
            .map_err(|reason| HookDeclarationError::Action {
                hook: hook.name.clone(),
                reason,
            })?;
    }
    Ok(())
}

/// Project inputs that a before-build hook declares, regardless of its option condition.
pub fn declared_generated_inputs(
    module: &LoadedModule,
) -> impl Iterator<Item = (String, PathBuf)> + '_ {
    module
        .hooks
        .before_build
        .iter()
        .filter(|hook| hook.enable)
        .flat_map(move |hook| {
            let producer = format!("@module/{}/{}", module.name, hook.name);
            hook.generates
                .iter()
                .map(move |path| (producer.clone(), expand_path(path, module.name.as_str())))
        })
}

/// Collect development hook paths without evaluating option conditions.
pub(crate) fn watch_inputs(
    project_root: &Path,
    module: &LoadedModule,
    inputs: &mut HookWatchInputs,
) -> Result<()> {
    for hook in module.hooks.build_hooks() {
        if !hook.enable || !hook.dev.participates_in_development() {
            continue;
        }
        if let crate::Action::Nu { path, .. } = hook.action().map_err(anyhow::Error::msg)? {
            inputs.hook_scripts.push(module.root.join(path));
        }
        if !hook.rerun_on.include.is_empty() {
            let sel = hook
                .rerun_on
                .expand(module.name.as_str())
                .compile_at(
                    project_root,
                    Some((module.name.as_str(), &module.root)),
                    Some(&module.owner),
                )
                .with_context(|| format!("the `{}` hook in module `{}`", hook.name, module.name))?;
            inputs.rerun_watches.push(crate::watch::RerunWatch {
                name: hook.name.to_owned(),
                manifest: module.manifest.clone(),
                stage: hook.stage,
                selection: sel,
            });
        }
        for path in hook.generates {
            let path = expand_path(path, module.name.as_str());
            relative(&path, "generates")?;
            inputs.generated_paths.push(project_root.join(path));
        }
    }
    Ok(())
}

fn eval_participation(
    alias: &str,
    hook: Hook<'_>,
    opts: &toml::Table,
) -> Result<HookParticipation> {
    if !hook.enable {
        return Ok(HookParticipation::Disabled);
    }
    let enabled = crate::options::enabled_by(hook.enable, hook.enabled_by, opts)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("module `{alias}` hook `{}`", hook.name))?;
    Ok(if enabled {
        HookParticipation::Enabled
    } else {
        HookParticipation::OptionDisabled
    })
}

fn topological_modules(modules: &BTreeMap<String, LoadedModule>) -> Vec<(&str, &LoadedModule)> {
    let mut ordered = Vec::with_capacity(modules.len());
    let mut visited = BTreeSet::new();
    fn visit<'a>(
        alias: &'a str,
        modules: &'a BTreeMap<String, LoadedModule>,
        visited: &mut BTreeSet<&'a str>,
        ordered: &mut Vec<(&'a str, &'a LoadedModule)>,
    ) {
        let Some(module) = modules.get(alias) else {
            return;
        };
        if !visited.insert(alias) {
            return;
        }
        for child_scoped in module.child_aliases.values() {
            visit(child_scoped.as_str(), modules, visited, ordered);
        }
        ordered.push((alias, module));
    }
    for alias in modules.keys() {
        visit(alias, modules, &mut visited, &mut ordered);
    }
    ordered
}

/// Evaluate module hook conditions and prepare declarations in dependency-first order.
pub fn hook_stages(
    project_root: &Path,
    modules: &BTreeMap<String, LoadedModule>,
) -> Result<HooksContribution> {
    let mut out = HooksContribution::default();
    for (alias, module) in topological_modules(modules) {
        for hook in module.hooks.build_hooks() {
            let name = format!("@module/{alias}/{}", hook.name);
            let participation = eval_participation(alias, hook, &module.options)?;
            let origin = ModuleHookOrigin {
                name: module.name.clone(),
                root: module.root.clone(),
                manifest: module.manifest.clone(),
                owner: module.owner.clone(),
                options_json: serde_json::to_vec(&module.options)?,
            };
            let rerun_on = hook.rerun_on.expand(alias);
            if participation == HookParticipation::Enabled
                && !hook.dev.participates_in_development()
                && !rerun_on.include.is_empty()
            {
                rerun_on
                    .compile(project_root, Some(&origin))
                    .with_context(|| format!("the `{name}` hook"))?;
            }
            let generates = hook
                .generates
                .iter()
                .map(|path| {
                    let path = expand_path(path, alias);
                    relative(&path, "generates")?;
                    Ok(path)
                })
                .collect::<Result<_>>()?;
            out.push(HookEntry {
                stage: hook.stage,
                name,
                declared_name: hook.name.to_owned(),
                order: hook.order,
                participation,
                origin,
                action: hook.action().map_err(anyhow::Error::msg)?,
                dev: hook.dev,
                rerun_on,
                generates,
                outputs: expand_outputs(hook.outputs, alias)?,
            });
        }
    }
    Ok(out)
}

/// The module whose manifest supplied a hook, retained by the execution plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHookOrigin {
    pub name: ModuleId,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub owner: ModuleOwner,
    pub options_json: Vec<u8>,
}

/// One declared generated file or exclusive generated directory tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandOutput {
    /// One exact output file.
    File(tola_address::OutputPath),
    /// An exclusively owned directory whose files may vary between invocations.
    Tree(tola_address::OutputPath),
}

impl CommandOutput {
    /// Logical path of the declared file or directory.
    pub fn path(&self) -> &tola_address::OutputPath {
        match self {
            Self::File(path) | Self::Tree(path) => path,
        }
    }
}

/// Point in the build lifecycle at which a hook executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HookStage {
    BeforeBuild,
    GenerateOutputs,
    AfterPublish,
}

impl HookStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeBuild => "before-build",
            Self::GenerateOutputs => "generate-outputs",
            Self::AfterPublish => "after-publish",
        }
    }
}

/// Whether an enabled finite command also runs in development.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DevParticipation {
    /// Run the command on every build that reaches its stage.
    #[default]
    Run,
    /// Leave the command out of the development session.
    ///
    /// Production builds still run it.
    Skip,
    /// Reuse a successful run's outputs in the same development session if its inputs are unchanged.
    ///
    /// Use with `before-build` and a nonempty `generates` list, or with `generate-outputs`.
    /// Declare the additional files the command reads in `rerun-on`. Use [`Self::Run`] for
    /// commands that depend on time, the network, or other untracked state.
    Reuse,
}

impl DevParticipation {
    pub const VALUES: &'static [&'static str] = &["\"run\"", "\"skip\""];
    pub const REUSE_VALUES: &'static [&'static str] = &["\"run\"", "\"skip\"", "\"reuse\""];

    /// Whether a development session runs or reuses this command at all.
    pub const fn participates_in_development(self) -> bool {
        !matches!(self, Self::Skip)
    }

    /// Whether this entry may reuse a previous development revision's outputs.
    pub const fn reuses_outputs(self) -> bool {
        matches!(self, Self::Reuse)
    }
}

#[cfg(test)]
mod tests {
    use super::{HookDeclarationError, ModuleHooks, validate_declarations};

    #[test]
    fn before_build_reuse_generates() {
        let src = "[[before-build]]\nname = 'prepare'\nnu = 'prepare.nu'\ndev = 'reuse'\n";
        let hooks: ModuleHooks = toml::from_str(src).unwrap();
        assert!(matches!(
            validate_declarations(&hooks),
            Err(HookDeclarationError::ReuseNeedsGenerates { .. })
        ));

        let hooks: ModuleHooks =
            toml::from_str(&format!("{src}generates = ['content/generated.typ']\n")).unwrap();
        assert!(validate_declarations(&hooks).is_ok());

        let hooks: ModuleHooks =
            toml::from_str(&src.replace("before-build", "after-publish")).unwrap();
        assert!(matches!(
            validate_declarations(&hooks),
            Err(HookDeclarationError::AfterPublishReuse { .. })
        ));
    }

    #[test]
    fn rerun_vcs_inputs() {
        use super::{RerunOn, RerunPath};

        let root = std::path::absolute("site").unwrap();
        let sel = RerunOn {
            include: [
                "**/*.typ",
                ".gitignore",
                ".git/HEAD",
                ".git/refs/*/main",
                ".jj/repo/store/**",
            ]
            .into_iter()
            .map(|p| RerunPath::Relative(p.to_owned()))
            .collect(),
            exclude: Vec::new(),
        }
        .compile(&root, None)
        .unwrap();
        for path in [".git/objects", ".jj/working_copy", "nested/.git/refs"] {
            assert!(!sel.could_match_below(&root.join(path)), "{path}");
        }
        for path in [
            ".git/HEAD",
            ".git/refs/heads/main",
            ".jj/repo/store/change",
            "docs/.hidden.typ",
            ".gitignore",
        ] {
            assert!(sel.matches(&root.join(path)), "{path}");
        }
        assert!(!sel.matches(&root.join(".git/refs/heads/other.typ")));
        assert!(!sel.matches(&root.join(".jj/working_copy/input.typ")));
    }

    #[test]
    fn hook_order_build_only() {
        let hooks: ModuleHooks =
            toml::from_str("[[before-build]]\nname = 'prepare'\norder = 4\nargv = ['tool']\n")
                .unwrap();
        assert_eq!(hooks.build_hooks().next().unwrap().order, 4);
        assert!(
            toml::from_str::<ModuleHooks>(
                "[[before-build]]\nname = 'prepare'\norder = -1\nargv = ['tool']\n"
            )
            .is_err()
        );
        assert!(
            toml::from_str::<ModuleHooks>(
                "[[before-pack]]\nname = 'prepare'\norder = 1\nargv = ['tool']\n"
            )
            .is_err()
        );
    }

    #[test]
    fn rerun_owner_and_dependency_hook_order() {
        use super::{RerunOn, RerunPath, hook_stages};
        use crate::identity::{ModuleAlias, ModuleId};
        use crate::module_set::{LoadedModule, ModuleOwner};
        use std::collections::BTreeMap;

        let dir = tempfile::tempdir().unwrap();
        let site_root = dir.path();
        let docs_root_raw = site_root.join("docs");
        std::fs::create_dir_all(&docs_root_raw).unwrap();
        let docs_root = docs_root_raw.canonicalize().unwrap();
        let tw_root = docs_root.join("tw");

        let origin = super::ModuleHookOrigin {
            name: ModuleId::parse("docs/tailwind").unwrap(),
            root: tw_root.clone(),
            manifest: tw_root.join("tola-module.toml"),
            owner: ModuleOwner::Module {
                id: ModuleId::parse("docs").unwrap(),
                root: docs_root.clone(),
            },
            options_json: b"{}".to_vec(),
        };
        let sel = RerunOn {
            include: vec![RerunPath::Owner {
                owner: "styles".into(),
            }],
            exclude: Vec::new(),
        }
        .compile(site_root, Some(&origin))
        .unwrap();
        assert!(sel.matches(&docs_root.join("styles/site.css")));
        assert!(!sel.matches(&site_root.join("styles/site.css")));

        let child_hooks: ModuleHooks =
            toml::from_str("[[before-build]]\nname = 'tw'\nargv = ['tw']\n").unwrap();
        let parent_hooks: ModuleHooks =
            toml::from_str("[[before-build]]\nname = 'doc'\nargv = ['doc']\n").unwrap();
        let mut child_aliases = BTreeMap::new();
        child_aliases.insert(
            ModuleAlias::parse("z").unwrap(),
            ModuleId::parse("a/z").unwrap(),
        );
        let modules = BTreeMap::from([
            (
                "a".to_owned(),
                LoadedModule {
                    name: ModuleId::parse("a").unwrap(),
                    root: docs_root.clone(),
                    manifest: docs_root.join("tola-module.toml"),
                    owner: ModuleOwner::Site,
                    source: None,
                    manifest_evidence: None,
                    min_tola_version: None,
                    typst: None,
                    nu: None,
                    build: None,
                    assets: None,
                    icons: Default::default(),
                    queries: BTreeMap::new(),
                    commands: BTreeMap::new(),
                    initializer: None,
                    hooks: parent_hooks,
                    modules: BTreeMap::new(),
                    child_aliases,
                    options: Default::default(),
                },
            ),
            (
                "a/z".to_owned(),
                LoadedModule {
                    name: ModuleId::parse("a/z").unwrap(),
                    root: tw_root.clone(),
                    manifest: tw_root.join("tola-module.toml"),
                    owner: ModuleOwner::Module {
                        id: ModuleId::parse("a").unwrap(),
                        root: docs_root,
                    },
                    source: None,
                    manifest_evidence: None,
                    min_tola_version: None,
                    typst: None,
                    nu: None,
                    build: None,
                    assets: None,
                    icons: Default::default(),
                    queries: BTreeMap::new(),
                    commands: BTreeMap::new(),
                    initializer: None,
                    hooks: child_hooks,
                    modules: BTreeMap::new(),
                    child_aliases: BTreeMap::new(),
                    options: Default::default(),
                },
            ),
        ]);
        let contrib = hook_stages(site_root, &modules).unwrap();
        let names: Vec<_> = contrib.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["@module/a/z/tw", "@module/a/doc"]);
    }
}
