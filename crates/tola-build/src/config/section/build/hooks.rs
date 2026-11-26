//! Finite commands around candidate construction and publication.
//!
//! The environment a hook command receives, and what a superseded build cancels, is written
//! for the site author in the section's and each stage's `HELP`, which `tola help "[hooks]"`
//! and its stage pages render.

use crate::config::{ConfigDiagnostics, FieldPath};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tola_config::Config;
use tola_module::Action;
use tola_module::hooks::RerunSelection;
pub use tola_module::hooks::{
    CommandOutput, DevParticipation, HookStage, ModuleHookOrigin, RerunOn,
};
use tola_module::options::OptionPath;

mod help;

/// Run scripts before compilation, while generating files, or after publication.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks")]
pub struct HooksConfig {
    #[serde(rename = "before-build")]
    #[config(sub, collection = array_table)]
    pub before_build: Vec<BeforeBuildHookConfig>,
    #[serde(rename = "generate-outputs")]
    #[config(sub, collection = array_table)]
    pub generate_outputs: Vec<OutputCommandConfig>,
    #[serde(rename = "after-publish")]
    #[config(sub, collection = array_table)]
    pub after_publish: Vec<AfterPublishHookConfig>,
}

/// Chain one projection over the three hook stages in lifecycle order.
macro_rules! hook_stages {
    ($cfg:expr, keep $keep:expr, map $proj:expr) => {
        $cfg.before_build
            .iter()
            .filter($keep)
            .map($proj)
            .chain($cfg.generate_outputs.iter().filter($keep).map($proj))
            .chain($cfg.after_publish.iter().filter($keep).map($proj))
    };
}

fn select_hook_option(
    enable: &mut bool,
    option_disabled: &mut bool,
    key: Option<&OptionPath>,
    options: &toml::Table,
    stage: HookStage,
    name: &str,
) -> anyhow::Result<()> {
    let enabled = tola_module::options::enabled_by(*enable, key, options)
        .map_err(|msg| anyhow::anyhow!("{}: {msg}", hook_identity(stage, name)))?;
    *option_disabled |= *enable && !enabled;
    *enable = enabled;
    Ok(())
}

impl HooksConfig {
    /// Enabled command arguments in lifecycle order, borrowed without copying.
    pub fn enabled_commands(&self) -> impl Iterator<Item = &[String]> {
        hook_stages!(
            self,
            keep | hook | hook.enable,
            map | hook | hook.argv.as_deref()
        )
        .flatten()
    }

    /// Additional input selections of the hooks that participate in development.
    pub fn development_rerun_selections(&self, root: &Path) -> anyhow::Result<Vec<RerunSelection>> {
        hook_stages!(
            self,
            keep | hook | hook.enable
                && hook.dev.participates_in_development()
                && !hook.rerun_on.include.is_empty(),
            map | hook | hook.rerun_on.compile(root, hook.origin.as_ref())
        )
        .collect()
    }

    /// Absolute Nu script paths of the hooks that participate in development.
    pub fn development_hook_scripts<'a>(
        &'a self,
        root: &'a Path,
    ) -> impl Iterator<Item = PathBuf> + 'a {
        hook_stages!(
            self,
            keep |hook| hook.enable && hook.dev.participates_in_development(),
            map move |hook| hook.nu.as_ref().map(|path| {
                hook.origin
                    .as_ref()
                    .map_or(root, |origin| origin.root.as_path())
                    .join(path)
            })
        )
        .flatten()
    }

    /// Select site hooks using the declaring site's boolean options.
    pub(crate) fn apply_site_options(&mut self, options: &toml::Table) -> anyhow::Result<()> {
        macro_rules! apply_stage {
            ($hooks:expr, $stage:expr) => {
                for hook in $hooks {
                    select_hook_option(
                        &mut hook.enable,
                        &mut hook.option_disabled,
                        hook.enabled_by.as_ref(),
                        options,
                        $stage,
                        &hook.name,
                    )?;
                }
            };
        }
        apply_stage!(&mut self.before_build, HookStage::BeforeBuild);
        apply_stage!(&mut self.generate_outputs, HookStage::GenerateOutputs);
        apply_stage!(&mut self.after_publish, HookStage::AfterPublish);
        Ok(())
    }

    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        validate_stage(
            &self.before_build,
            Self::FIELDS.before_build.as_str(),
            HookEntryFields::BEFORE_BUILD,
            diag,
            |hook, idx, diag| {
                let generates = BeforeBuildHookConfig::FIELDS.generates;
                if hook.dev.reuses_outputs() && hook.generates.is_empty() {
                    diag.error_with_help(
                        generates,
                        format!(
                            "`{}` is required with `dev = \"reuse\"`",
                            message_key(generates, idx)
                        ),
                        "declare the files or directories this hook generates",
                    );
                }
                for (entry_idx, generated) in hook.generates.iter().enumerate() {
                    validate_site_relative(generated, idx, entry_idx, generates, diag);
                }
            },
        );

        let outputs = OutputCommandConfig::FIELDS.outputs;
        validate_stage(
            &self.generate_outputs,
            Self::FIELDS.generate_outputs.as_str(),
            HookEntryFields::GENERATE_OUTPUTS,
            diag,
            |cmd, idx, diag| {
                if cmd.outputs.is_empty() {
                    diag.error_with_help(
                        outputs,
                        format!("`{}` is empty", message_key(outputs, idx)),
                        "declare each output with `{ file = … }` or `{ tree = … }`",
                    );
                }
                let mut paths = std::collections::BTreeSet::new();
                for out in &cmd.outputs {
                    let path = out.path();
                    if path.is_reserved_for_non_system_output() {
                        diag.error_with_help(
                            outputs,
                            format!("`{path}` is reserved"),
                            "choose a path outside `_tola`",
                        );
                    }
                    if !paths.insert(path) {
                        diag.error_with_help(
                            outputs,
                            format!("`{}` declares `{path}` twice", message_key(outputs, idx)),
                            "keep one entry",
                        );
                    }
                }
            },
        );

        validate_stage(
            &self.after_publish,
            Self::FIELDS.after_publish.as_str(),
            HookEntryFields::AFTER_PUBLISH,
            diag,
            |hook, idx, diag| {
                if hook.dev.reuses_outputs() {
                    let dev = AfterPublishHookConfig::FIELDS.dev;
                    diag.error_with_help(
                        dev,
                        format!("`{}` does not accept `\"reuse\"`", message_key(dev, idx)),
                        "use `\"run\"` or `\"skip\"`",
                    );
                }
            },
        );
    }
}

/// The declared field paths of one entry, per hook stage.
#[derive(Clone, Copy)]
struct HookEntryFields {
    name: FieldPath,
    argv: FieldPath,
    rerun_on: FieldPath,
}

impl HookEntryFields {
    const BEFORE_BUILD: Self = Self {
        name: BeforeBuildHookConfig::FIELDS.name,
        argv: BeforeBuildHookConfig::FIELDS.argv,
        rerun_on: BeforeBuildHookConfig::FIELDS.rerun_on,
    };

    const GENERATE_OUTPUTS: Self = Self {
        name: OutputCommandConfig::FIELDS.name,
        argv: OutputCommandConfig::FIELDS.argv,
        rerun_on: OutputCommandConfig::FIELDS.rerun_on,
    };

    const AFTER_PUBLISH: Self = Self {
        name: AfterPublishHookConfig::FIELDS.name,
        argv: AfterPublishHookConfig::FIELDS.argv,
        rerun_on: AfterPublishHookConfig::FIELDS.rerun_on,
    };
}

trait HookEntryMeta {
    fn name(&self) -> &str;
    fn action_result(&self) -> Result<Action, String>;
    fn rerun_on(&self) -> &RerunOn;
}

/// Spell an entry field key with its array index, e.g. `hooks.before-build[1].argv`.
fn message_key(field: FieldPath, idx: usize) -> String {
    match field.as_str().rsplit_once('.') {
        Some((array, name)) => format!("{array}[{idx}].{name}"),
        None => field.as_str().to_owned(),
    }
}

fn validate_stage<T: HookEntryMeta>(
    entries: &[T],
    array_key: &'static str,
    fields: HookEntryFields,
    diag: &mut ConfigDiagnostics,
    extra: impl Fn(&T, usize, &mut ConfigDiagnostics),
) {
    let mut named = Vec::new();
    for (idx, entry) in entries.iter().enumerate() {
        diag.with_array_element(array_key, idx, |diag| {
            validate_command(
                entry.name(),
                entry.action_result(),
                entry.rerun_on(),
                idx,
                fields,
                diag,
            );
            validate_name_is_unique(&mut named, entry.name(), idx, fields.name, diag);
            extra(entry, idx, diag);
        });
    }
}

/// Runs before source discovery to write the files the site then reads: Typst's inputs, and the
/// files `[assets]` declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.before-build", collection = array_table)]
pub struct BeforeBuildHookConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// The module-local name, when this hook belongs to a module.
    #[serde(skip)]
    #[config(skip)]
    pub declared_name: Option<String>,
    /// Whether the hook was disabled by its option.
    #[serde(skip)]
    #[config(skip)]
    pub option_disabled: bool,
    /// Set `false` to turn off this hook. If omitted, the hook is enabled.
    pub enable: bool,
    /// Name shown in progress and diagnostics. Choose a nonempty name with no whitespace,
    /// unique within this site or module instance and stage.
    pub name: String,
    /// Execution batch within this site or module instance and stage. Defaults to 0.
    /// Lower values finish first; equal values may run together and must be independent.
    pub order: u64,
    /// Command and arguments to run from the site root. Use `argv` or `nu`. To run a shell
    /// script, name the shell explicitly. Write files where the site will read them.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Nu script to run, relative to the site root. Use `nu` or `argv`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Arguments passed to the Nu script. Omit this for a script that takes no arguments.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Run only when this `[options]` key is `true`, for example `images.enable`.
    /// Omit this setting to run without checking an option.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    #[config(fragment_with = crate::config::fragment::option_path)]
    pub enabled_by: Option<OptionPath>,
    /// If omitted or set to `"run"`, `tola dev` runs the hook on each build. With `"reuse"`,
    /// it can keep unchanged files listed in `generates`. Use `"skip"` to skip it.
    #[config(values = DevParticipation::REUSE_VALUES)]
    #[config(fragment_with = tola_config::validate_value::<DevParticipation>)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns.
    #[serde(rename = "rerun-on")]
    #[config(fragment_with = crate::config::fragment::rerun_on)]
    pub rerun_on: RerunOn,
    /// Files or directories the hook's command generates, relative to the site root; they must not
    /// be inside `build.publish-dir` or `.tola`. After the hook's command, Tola checks that each
    /// declared path exists: the hook declares what it wrote, and Tola does not infer it. Declaring
    /// it does not publish it: publish it through `[assets]` or a Bundle `asset(...)`, or use it to
    /// produce a document or another output.
    #[config(collection = inline)]
    pub generates: Vec<PathBuf>,
}

/// Generate files after Typst compiles. Use `before-build` for files Typst needs to read.
/// Tola checks links to generated output before publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.generate-outputs", collection = array_table)]
pub struct OutputCommandConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// The module-local name, when this hook belongs to a module.
    #[serde(skip)]
    #[config(skip)]
    pub declared_name: Option<String>,
    /// Whether the hook was disabled by its option.
    #[serde(skip)]
    #[config(skip)]
    pub option_disabled: bool,
    /// Set `false` to turn off this hook. If omitted, the hook is enabled.
    pub enable: bool,
    /// Name shown in progress and diagnostics. Choose a nonempty name with no whitespace,
    /// unique within this site or module instance and stage.
    pub name: String,
    /// Execution batch within this site or module instance and stage. Defaults to 0.
    /// Lower values finish first; equal values may run together and must be independent.
    pub order: u64,
    /// Command and arguments to run from the site root. Use `argv` or `nu`. Read compiled pages
    /// and assets from `hook.inputDir`. Write files to `hook.outputDir` using
    /// paths relative to the publish directory.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Nu script to run, relative to the site root. Use `nu` or `argv`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Arguments passed to the Nu script. Omit this for a script that takes no arguments.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Run only when this `[options]` key is `true`, for example `images.enable`.
    /// Omit this setting to run without checking an option.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    #[config(fragment_with = crate::config::fragment::option_path)]
    pub enabled_by: Option<OptionPath>,
    /// If omitted or set to `"run"`, `tola dev` runs the hook on each build. Use `"reuse"`
    /// to use the last successful outputs when the inputs are unchanged, or `"skip"`
    /// to skip the hook during development.
    #[config(values = DevParticipation::REUSE_VALUES)]
    #[config(fragment_with = tola_config::validate_value::<DevParticipation>)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns. For
    /// `dev = "reuse"`, the selected inputs are also part of the hook's fingerprint.
    #[serde(rename = "rerun-on")]
    #[config(fragment_with = crate::config::fragment::rerun_on)]
    pub rerun_on: RerunOn,
    /// Required output paths relative to `build.publish-dir`, for example
    /// `{ file = "build.txt" }` or `{ tree = "assets/search" }`. Written files must match
    /// these declarations. Keep them outside `_tola` and use unique output paths.
    #[config(collection = inline, fragment_with = crate::config::fragment::command_outputs)]
    pub outputs: Vec<CommandOutput>,
}

/// Run a command with a read-only copy of the published site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "hooks.after-publish", collection = array_table)]
pub struct AfterPublishHookConfig {
    #[serde(skip)]
    #[config(skip)]
    pub origin: Option<ModuleHookOrigin>,
    /// The module-local name, when this hook belongs to a module.
    #[serde(skip)]
    #[config(skip)]
    pub declared_name: Option<String>,
    /// Whether the hook was disabled by its option.
    #[serde(skip)]
    #[config(skip)]
    pub option_disabled: bool,
    /// Set `false` to turn off this hook. If omitted, the hook is enabled.
    pub enable: bool,
    /// Name shown in progress and diagnostics. Choose a nonempty name with no whitespace,
    /// unique within this site or module instance and stage.
    pub name: String,
    /// Execution batch within this site or module instance and stage. Defaults to 0.
    /// Lower values finish first; equal values may run together and must be independent.
    pub order: u64,
    /// Command and arguments to run from the site root. Use `argv` or `nu`.
    /// Read the published files from `hook.inputDir`. If the command fails, the site is
    /// already published.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Nu script to run, relative to the site root. Use `nu` or `argv`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nu: Option<PathBuf>,
    /// Arguments passed to the Nu script. Omit this for a script that takes no arguments.
    #[config(collection = inline)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Run only when this `[options]` key is `true`, for example `images.enable`.
    /// Omit this setting to run without checking an option.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    #[config(fragment_with = crate::config::fragment::option_path)]
    pub enabled_by: Option<OptionPath>,
    /// If omitted or set to `"skip"`, `tola dev` skips the hook. Use `"run"` to run it
    /// after a successful development build.
    #[config(values = DevParticipation::VALUES)]
    #[config(fragment_with = tola_config::validate_value::<DevParticipation>)]
    pub dev: DevParticipation,
    /// Extra site-relative inputs selected by `include` and `exclude` patterns.
    #[serde(rename = "rerun-on")]
    #[config(fragment_with = crate::config::fragment::rerun_on)]
    pub rerun_on: RerunOn,
}

impl Default for BeforeBuildHookConfig {
    fn default() -> Self {
        Self {
            origin: None,
            declared_name: None,
            option_disabled: false,
            enable: true,
            name: String::new(),
            order: 0,
            argv: None,
            nu: None,
            args: None,
            enabled_by: None,
            dev: DevParticipation::Run,
            rerun_on: RerunOn::default(),
            generates: Vec::new(),
        }
    }
}

impl Default for OutputCommandConfig {
    fn default() -> Self {
        Self {
            origin: None,
            declared_name: None,
            option_disabled: false,
            enable: true,
            name: String::new(),
            order: 0,
            argv: None,
            nu: None,
            args: None,
            enabled_by: None,
            dev: DevParticipation::Run,
            rerun_on: RerunOn::default(),
            outputs: Vec::new(),
        }
    }
}

impl Default for AfterPublishHookConfig {
    fn default() -> Self {
        Self {
            origin: None,
            declared_name: None,
            option_disabled: false,
            enable: true,
            name: String::new(),
            order: 0,
            argv: None,
            nu: None,
            args: None,
            enabled_by: None,
            dev: DevParticipation::Skip,
            rerun_on: RerunOn::default(),
        }
    }
}

macro_rules! hook_actions {
    ($($entry:ty),+ $(,)?) => {
        $(
            impl $entry {
                /// Validate and return this hook's executable action.
                pub fn action(&self) -> Result<Action, String> {
                    Action::from_fields(self.nu.clone(), self.argv.clone(), self.args.clone())
                }

                /// Set the already validated module action on a build hook.
                pub(crate) fn set_action(&mut self, action: Action) {
                    match action {
                        Action::Nu { path, args } => {
                            self.argv = None;
                            self.nu = Some(path);
                            self.args = Some(args);
                        }
                        Action::Argv(argv) => {
                            self.argv = Some(argv);
                            self.nu = None;
                            self.args = None;
                        }
                    }
                }
            }

            impl HookEntryMeta for $entry {
                fn name(&self) -> &str {
                    &self.name
                }
                fn action_result(&self) -> Result<Action, String> {
                    self.action()
                }
                fn rerun_on(&self) -> &RerunOn {
                    &self.rerun_on
                }
            }
        )+
    };
}

hook_actions!(
    BeforeBuildHookConfig,
    OutputCommandConfig,
    AfterPublishHookConfig
);

fn validate_command(
    name: &str,
    action: Result<Action, String>,
    rerun_on: &RerunOn,
    idx: usize,
    fields: HookEntryFields,
    diag: &mut ConfigDiagnostics,
) {
    if let Err(reason) = action {
        let key = message_key(fields.argv, idx);
        diag.error_with_help(
            fields.argv,
            format!("`{key}`: {reason}"),
            "provide a `nu` script or an `argv` array",
        );
    }
    validate_name(name, idx, fields.name, diag);
    for (list, entry, err) in rerun_on.validate(false) {
        let field = fields.rerun_on;
        let key = message_key(field, idx);
        diag.error_with_help(
            field,
            format!("`{key}.{list}[{entry}]`: {err}"),
            "write a path or glob relative to the site root",
        );
    }
}

pub(crate) fn hook_identity(stage: HookStage, name: &str) -> String {
    format!("the `{name}` hook in `hooks.{}`", stage.as_str())
}

fn validate_name(name: &str, idx: usize, field: FieldPath, diag: &mut ConfigDiagnostics) {
    let key = message_key(field, idx);
    if name.trim().is_empty() {
        diag.error_with_help(
            field,
            format!("`{key}` is blank"),
            "name the command, such as `name = \"css-build\"`",
        );
    } else if name.chars().any(char::is_whitespace) {
        diag.error_with_help(
            field,
            format!("`{key}` is not one word"),
            "Remove the whitespace, as in `css-build`",
        );
    } else if name.starts_with("@module/") {
        diag.error_with_help(
            field,
            format!("`{key}` starts with reserved `@module/`"),
            "choose a name for the site hook",
        );
    }
}

fn validate_name_is_unique<'a>(
    named: &mut Vec<(&'a str, usize)>,
    name: &'a str,
    idx: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    if name.trim().is_empty() {
        return;
    }
    if let Some((_, first)) = named.iter().find(|(seen, _)| *seen == name) {
        diag.error_with_help(
            field,
            format!(
                "`{}` repeats `{}`",
                message_key(field, idx),
                message_key(field, *first)
            ),
            "give each entry its own name",
        );
        return;
    }
    named.push((name, idx));
}

fn validate_site_relative(
    path: &Path,
    hook_idx: usize,
    entry_idx: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    let key = format!("{}[{entry_idx}]", message_key(field, hook_idx));
    // A hook names a file or directory: the site root itself is not one.
    if path == Path::new(".") {
        diag.error_with_help(
            field,
            format!("`{key}` is `.`"),
            "name a file or directory inside the site root",
        );
        return;
    }
    crate::config::section::path::validate_site_relative_path(path, &key, field, diag);
}

#[cfg(test)]
mod tests;
