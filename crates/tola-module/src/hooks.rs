//! Module hook declarations and the build hooks selected for each instance.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use tola_input::{FilePatterns, FileSelection};

use crate::module_set::LoadedModule;
use crate::prepared::{HookEntry, HooksContribution};
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
            #[serde(default = "enabled")]
            enable: bool,
            nu: Option<PathBuf>,
            argv: Option<Vec<String>>,
            args: Option<Vec<String>>,
            enabled_by: Option<String>,
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
                    enable: self.enable,
                    nu: &self.nu,
                    argv: &self.argv,
                    args: &self.args,
                    enabled_by: self.enabled_by.as_deref(),
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
    enable: bool,
    nu: &'a Option<PathBuf>,
    argv: &'a Option<Vec<String>>,
    args: &'a Option<Vec<String>>,
    enabled_by: Option<&'a str>,
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

/// One path or glob relative to its declaring root or explicitly to the project.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum RerunPath {
    Relative(String),
    Project { project: String },
}

impl RerunPath {
    fn pattern(&self) -> &str {
        match self {
            Self::Relative(pattern) | Self::Project { project: pattern } => pattern,
        }
    }

    fn validate(&self, module: bool, include: bool) -> Result<()> {
        let pattern = self.pattern();
        let pattern = if module {
            ensure!(
                !["{module}", "{project}", "{tola}"]
                    .iter()
                    .any(|&name| pattern.contains(name)),
                "the pattern `{pattern}` only accepts the `{{name}}` substitution"
            );
            pattern.replace("{name}", "module")
        } else {
            pattern.to_owned()
        };
        let mut patterns = FilePatterns::default();
        if include {
            patterns.include.push(pattern);
        } else {
            patterns.exclude.push(pattern);
        }
        FileSelection::new(&patterns).map_err(|error| error.at("rerun-on"))?;
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
            .flat_map(move |(include, patterns)| {
                let key = if include { "include" } else { "exclude" };
                patterns
                    .iter()
                    .enumerate()
                    .filter_map(move |(index, pattern)| {
                        pattern
                            .validate(module, include)
                            .err()
                            .map(|error| (key, index, error))
                    })
            })
    }

    fn expand(&self, alias: &str) -> Self {
        let expand = |entry: &RerunPath| match entry {
            RerunPath::Relative(pattern) => RerunPath::Relative(pattern.replace("{name}", alias)),
            RerunPath::Project { project } => RerunPath::Project {
                project: project.replace("{name}", alias),
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
        let module = origin.map(|origin| (origin.name.as_str(), origin.root.as_path()));
        self.compile_at(site_root, module)
    }

    fn compile_at(
        &self,
        site_root: &Path,
        module: Option<(&str, &Path)>,
    ) -> Result<RerunSelection> {
        let mut site_patterns = FilePatterns::default();
        let mut module_patterns = FilePatterns::default();
        for (patterns, site, local) in [
            (
                &self.include,
                &mut site_patterns.include,
                &mut module_patterns.include,
            ),
            (
                &self.exclude,
                &mut site_patterns.exclude,
                &mut module_patterns.exclude,
            ),
        ] {
            for pattern in patterns {
                if module.is_some() && matches!(pattern, RerunPath::Relative(_)) {
                    local.push(pattern.pattern().to_owned());
                } else {
                    site.push(pattern.pattern().to_owned());
                }
            }
        }
        let mut scopes = Vec::new();
        if !site_patterns.include.is_empty() || !site_patterns.exclude.is_empty() {
            scopes.push((site_root.to_owned(), FileSelection::new(&site_patterns)?));
        }
        if let Some((alias, root)) = module
            && (!module_patterns.include.is_empty() || !module_patterns.exclude.is_empty())
        {
            let selection = FileSelection::new(&module_patterns)?;
            for path in selection.roots() {
                ensure!(
                    tola_input::normalize_existing_prefix(&root.join(&path)).starts_with(root),
                    "the `rerun-on` path `{}` leaves module `{alias}`",
                    path.display()
                );
            }
            scopes.push((root.to_owned(), selection));
        }
        Ok(RerunSelection { scopes })
    }
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
                .is_ok_and(|path| selection.includes(path))
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
                    selection.could_match_below(path)
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
    #[error("hook `{hook}` names no option for `enabled-by`")]
    EmptyEnabledBy { hook: String },
    #[error("hook `{hook}` can reuse its previous run only in `generate-outputs`")]
    ReuseNeedsGenerateOutputs { hook: String },
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
    let spelling = path.to_string_lossy();
    if spelling.contains("{module}")
        || spelling.contains("{project}")
        || spelling.contains("{tola}")
    {
        return Err(HookDeclarationError::Substitution { label });
    }
    if !(crate::is_relative_path(path)
        && path
            .components()
            .any(|part| matches!(part, Component::Normal(_))))
    {
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
        .map(|output| {
            let path =
                tola_address::OutputPath::parse(&output.path().as_str().replace("{name}", alias))?;
            ensure!(
                !path.is_reserved_for_non_system_output(),
                "`{path}` is reserved for Tola"
            );
            ensure!(
                seen.insert(path.clone()),
                "the hook generates `{path}` twice after `{{name}}` substitution"
            );
            Ok(match output {
                CommandOutput::File(_) => CommandOutput::File(path),
                CommandOutput::Tree(_) => CommandOutput::Tree(path),
            })
        })
        .collect()
}

/// Validate all declarations, including those disabled by options.
pub fn validate_declarations(hooks: &ModuleHooks) -> std::result::Result<(), HookDeclarationError> {
    fn validate_name<'a>(
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
    let mut names = BTreeSet::new();
    for hook in hooks.build_hooks() {
        validate_name(&mut names, Some(hook.stage), hook.name)?;
        hook.action()
            .map_err(|reason| HookDeclarationError::Action {
                hook: hook.name.into(),
                reason,
            })?;
        if hook
            .enabled_by
            .is_some_and(|option| option.trim().is_empty())
        {
            return Err(HookDeclarationError::EmptyEnabledBy {
                hook: hook.name.into(),
            });
        }
        if hook.dev == DevParticipation::Reuse && hook.stage != HookStage::GenerateOutputs {
            return Err(HookDeclarationError::ReuseNeedsGenerateOutputs {
                hook: hook.name.into(),
            });
        }
        if let Some((key, _, error)) = hook.rerun_on.validate(true).next() {
            return Err(HookDeclarationError::RerunPattern {
                hook: hook.name.to_owned(),
                key,
                reason: error.to_string(),
            });
        }
        for path in hook.generates {
            relative(path, "generates")?;
        }
        if hook.stage == HookStage::GenerateOutputs && hook.outputs.is_empty() {
            return Err(HookDeclarationError::GenerateOutputsNoOutputs);
        }
        let mut seen = BTreeSet::new();
        for output in hook.outputs {
            let path = output.path().as_str();
            if path.contains("{module}") || path.contains("{project}") || path.contains("{tola}") {
                return Err(HookDeclarationError::OutputSubstitution);
            }
            if output.path().is_reserved_for_non_system_output() {
                return Err(HookDeclarationError::OutputReserved { path: path.into() });
            }
            if !seen.insert(output.path()) {
                return Err(HookDeclarationError::OutputRepeated { path: path.into() });
            }
        }
    }
    for hook in &hooks.before_pack {
        validate_name(&mut names, None, &hook.name)?;
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
                .map(move |path| (producer.clone(), expand_path(path, &module.name)))
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
            let selection = hook
                .rerun_on
                .expand(&module.name)
                .compile_at(project_root, Some((&module.name, &module.root)))
                .with_context(|| format!("the `{}` hook in module `{}`", hook.name, module.name))?;
            inputs.rerun_selections.push(selection);
        }
        for path in hook.generates {
            let path = expand_path(path, &module.name);
            relative(&path, "generates")?;
            inputs.generated_paths.push(project_root.join(path));
        }
    }
    Ok(())
}

/// Evaluate module hook conditions and prepare active entries in module alias order.
pub fn hook_stages(
    project_root: &Path,
    modules: &BTreeMap<String, LoadedModule>,
) -> Result<HooksContribution> {
    let mut result = HooksContribution::default();
    for (alias, module) in modules {
        for hook in module.hooks.build_hooks().filter(|hook| hook.enable) {
            let name = format!("@module/{alias}/{}", hook.name);
            if let Some(key) = hook.enabled_by {
                match module.options.get(key) {
                    Some(toml::Value::Boolean(false)) => continue,
                    Some(toml::Value::Boolean(true)) => {}
                    Some(_) => bail!("hook `{name}`: option `{key}` must be a boolean"),
                    None => bail!("hook `{name}` is enabled by unset option `{key}`"),
                }
            }
            let origin = ModuleHookOrigin {
                name: alias.clone(),
                root: module.root.clone(),
                manifest: module.manifest.clone(),
                options_json: serde_json::to_vec(&module.options)?,
            };
            let rerun_on = hook.rerun_on.expand(alias);
            if !hook.dev.participates_in_development() && !rerun_on.include.is_empty() {
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
            result.push(HookEntry {
                stage: hook.stage,
                name,
                origin,
                action: hook.action().map_err(anyhow::Error::msg)?,
                dev: hook.dev,
                rerun_on,
                generates,
                outputs: expand_outputs(hook.outputs, alias)?,
            });
        }
    }
    Ok(result)
}

/// The module whose manifest supplied a hook, retained by the execution plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHookOrigin {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
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
    /// Reuse the previous development revision's outputs when the hook's inputs are unchanged.
    ///
    /// Only `generate-outputs` accepts this value. The command must be a pure function of
    /// its built-site input, its `rerun-on` paths, its command, and its declared outputs;
    /// a command that reads time, network, or other external state must use [`Self::Run`].
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
