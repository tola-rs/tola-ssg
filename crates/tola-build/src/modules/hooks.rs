//! Static module hook declarations and the executable hook stages they configure.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use typst::foundations::{Dict, Value};

use super::ResolvedModule;
use crate::config::section::build::hooks::{
    AfterPublishHookConfig, BeforeBuildHookConfig, CommandOutput, DevParticipation, HooksConfig,
    ModuleHookOrigin, OutputCommandConfig,
};

/// One hook declared by a module manifest. Stage-specific fields are validated on manifest load.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ModuleHook {
    stage: Stage,
    name: String,
    command: Vec<String>,
    enabled_by: Option<String>,
    dev: Option<DevParticipation>,
    #[serde(default)]
    rerun_on: Vec<RerunPath>,
    #[serde(default)]
    options: Vec<String>,
    generates: Option<Vec<PathBuf>>,
    outputs: Option<Vec<CommandOutput>>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
enum Stage {
    BeforeBuild,
    GenerateOutputs,
    AfterPublish,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum RerunPath {
    Module(PathBuf),
    Project(PathBuf),
}

/// A hook declaration a module manifest cannot use.
#[derive(Debug, thiserror::Error)]
pub enum HookDeclarationError {
    #[error("a hook name must be one word")]
    Name,
    #[error("hook `{name}` is declared twice in one stage")]
    DuplicateName { name: String },
    #[error("hook `{hook}` needs a `command`")]
    MissingExecutable { hook: String },
    #[error("hook `{hook}` names no option for `enabled-by`")]
    EmptyEnabledBy { hook: String },
    #[error("hook `{hook}` lists an empty `options` entry")]
    EmptyOptionName { hook: String },
    #[error("hook `{hook}` lists `{option}` twice in `options`")]
    RepeatedOption { hook: String, option: String },
    #[error("hook `{hook}` can reuse its previous run only in `generate-outputs`")]
    ReuseNeedsGenerateOutputs { hook: String },
    #[error("`{label}` only accepts the `{{name}}` substitution")]
    Substitution { label: &'static str },
    #[error("`{label}` must be a relative path inside its module or the site")]
    Path { label: &'static str },
    #[error("a `before-build` hook cannot declare outputs")]
    BeforeBuildOutputs,
    #[error("a `generate-outputs` hook cannot declare generates")]
    GenerateOutputsGenerates,
    #[error("a `generate-outputs` hook declares no outputs")]
    GenerateOutputsNoOutputs,
    #[error("an output only accepts the `{{name}}` substitution")]
    OutputSubstitution,
    #[error("`{path}` is reserved for Tola")]
    OutputReserved { path: String },
    #[error("the hook declares `{path}` twice")]
    OutputRepeated { path: String },
    #[error("an `after-publish` hook cannot declare generates or outputs")]
    AfterPublishFields,
}

fn relative(path: &Path, label: &'static str) -> std::result::Result<(), HookDeclarationError> {
    let spelling = path.to_string_lossy();
    if spelling.contains("{module}") || spelling.contains("{project}") {
        return Err(HookDeclarationError::Substitution { label });
    }
    if !(super::is_relative_path(path)
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

/// A project path stays relative; a module path is absolute inside the module.
fn resolve_rerun_path(
    module: &ResolvedModule,
    alias: &str,
    hook: &str,
    path: &RerunPath,
) -> Result<PathBuf> {
    match path {
        RerunPath::Module(path) => {
            let resolved = module.root.join(path);
            ensure!(
                crate::filesystem::normalize_existing_prefix(&resolved).starts_with(&module.root),
                "the `rerun-on` path `{}` of hook `{hook}` in module `{alias}` leaves the module",
                path.display()
            );
            Ok(resolved)
        }
        RerunPath::Project(path) => {
            let path = expand_path(path, alias);
            relative(&path, "rerun-on")?;
            Ok(path)
        }
    }
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
pub(super) fn validate_declarations(
    hooks: &[ModuleHook],
) -> std::result::Result<(), HookDeclarationError> {
    let mut names = BTreeSet::new();
    for hook in hooks {
        if hook.name.is_empty() || hook.name.chars().any(char::is_whitespace) {
            return Err(HookDeclarationError::Name);
        }
        if !names.insert((hook.stage, &hook.name)) {
            return Err(HookDeclarationError::DuplicateName {
                name: hook.name.clone(),
            });
        }
        if hook.command.is_empty() || hook.command[0].trim().is_empty() {
            return Err(HookDeclarationError::MissingExecutable {
                hook: hook.name.clone(),
            });
        }
        if hook
            .enabled_by
            .as_ref()
            .is_some_and(|option| option.trim().is_empty())
        {
            return Err(HookDeclarationError::EmptyEnabledBy {
                hook: hook.name.clone(),
            });
        }
        let mut option_names = BTreeSet::new();
        for name in &hook.options {
            if name.trim().is_empty() {
                return Err(HookDeclarationError::EmptyOptionName {
                    hook: hook.name.clone(),
                });
            }
            if !option_names.insert(name) {
                return Err(HookDeclarationError::RepeatedOption {
                    hook: hook.name.clone(),
                    option: name.clone(),
                });
            }
        }
        if hook.dev == Some(DevParticipation::Reuse) && hook.stage != Stage::GenerateOutputs {
            return Err(HookDeclarationError::ReuseNeedsGenerateOutputs {
                hook: hook.name.clone(),
            });
        }
        for path in &hook.rerun_on {
            match path {
                RerunPath::Module(path) | RerunPath::Project(path) => relative(path, "rerun-on")?,
            }
        }
        match hook.stage {
            Stage::BeforeBuild => {
                if hook.outputs.is_some() {
                    return Err(HookDeclarationError::BeforeBuildOutputs);
                }
                for path in hook.generates.iter().flatten() {
                    relative(path, "generates")?;
                }
            }
            Stage::GenerateOutputs => {
                if hook.generates.is_some() {
                    return Err(HookDeclarationError::GenerateOutputsGenerates);
                }
                let outputs = hook
                    .outputs
                    .as_ref()
                    .filter(|outputs| !outputs.is_empty())
                    .ok_or(HookDeclarationError::GenerateOutputsNoOutputs)?;
                let mut seen = BTreeSet::new();
                for output in outputs {
                    let path = output.path().as_str();
                    if path.contains("{module}") || path.contains("{project}") {
                        return Err(HookDeclarationError::OutputSubstitution);
                    }
                    if output.path().is_reserved_for_non_system_output() {
                        return Err(HookDeclarationError::OutputReserved {
                            path: path.to_owned(),
                        });
                    }
                    if !seen.insert(output.path()) {
                        return Err(HookDeclarationError::OutputRepeated {
                            path: path.to_owned(),
                        });
                    }
                }
            }
            Stage::AfterPublish => {
                if hook.generates.is_some() || hook.outputs.is_some() {
                    return Err(HookDeclarationError::AfterPublishFields);
                }
            }
        }
    }
    Ok(())
}

/// Project inputs that a before-build hook declares, regardless of its option condition.
/// Queries report these as unverified rather than evaluating options or running hooks.
pub(super) fn declared_generated_inputs(
    module: &ResolvedModule,
) -> impl Iterator<Item = (String, PathBuf)> + '_ {
    module
        .hooks
        .iter()
        .filter(|hook| hook.stage == Stage::BeforeBuild)
        .filter_map(|hook| hook.generates.as_ref().map(|paths| (hook, paths)))
        .flat_map(move |(hook, paths)| {
            let producer = format!("@module/{}/{}", module.name, hook.name);
            paths
                .iter()
                .map(move |path| (producer.clone(), expand_path(path, &module.name)))
        })
}

/// Collect possible development hook paths without evaluating options or running commands.
pub(super) fn watch_inputs(
    project_root: &Path,
    module: &ResolvedModule,
    rerun_paths: &mut Vec<PathBuf>,
    generated_paths: &mut Vec<PathBuf>,
) -> Result<()> {
    for hook in &module.hooks {
        let default = if hook.stage == Stage::AfterPublish {
            DevParticipation::Skip
        } else {
            DevParticipation::Run
        };
        if !hook.dev.unwrap_or(default).participates_in_development() {
            continue;
        }
        for path in &hook.rerun_on {
            // A module path is already absolute, so joining it to the project root leaves it in
            // the module.
            let resolved = resolve_rerun_path(module, &module.name, &hook.name, path)?;
            rerun_paths.push(project_root.join(resolved));
        }
        if hook.stage == Stage::BeforeBuild {
            for path in hook.generates.iter().flatten() {
                let path = expand_path(path, &module.name);
                relative(&path, "generates")?;
                generated_paths.push(project_root.join(path));
            }
        }
    }
    Ok(())
}

/// Only module hooks; callers prepend each stage to the project hooks.
pub(crate) fn hook_stages(
    modules: &BTreeMap<String, ResolvedModule>,
    options: &BTreeMap<String, Dict>,
) -> Result<HooksConfig> {
    let mut result = HooksConfig::default();
    for (alias, module) in modules {
        validate_declarations(&module.hooks).map_err(|error| {
            anyhow::anyhow!("module `{alias}` declares an invalid hook: {error}")
        })?;
        for hook in &module.hooks {
            let hook_name = &hook.name;
            if let Some(key) = &hook.enabled_by {
                let value = options
                    .get(alias)
                    .and_then(|options| options.get(key.as_str()).ok())
                    .with_context(|| {
                        format!(
                            "module `{alias}` hook `{hook_name}` is enabled by `{key}`, which its options do not set"
                        )
                    })?;
                match value {
                    Value::Bool(false) => continue,
                    Value::Bool(true) => {}
                    _ => bail!("module `{alias}` hook `{hook_name}`: `{key}` must be a boolean"),
                }
            }
            let mut projected = serde_json::Map::new();
            for key in &hook.options {
                let value = options
                    .get(alias)
                    .and_then(|values| values.get(key.as_str()).ok())
                    .with_context(|| {
                        format!(
                            "module `{alias}` hook `{hook_name}` reads `{key}`, which its options do not set"
                        )
                    })?;
                projected.insert(
                    key.clone(),
                    super::query::json_value(value, key).with_context(|| {
                        format!(
                            "module `{alias}` hook `{hook_name}` cannot read its option `{key}`"
                        )
                    })?,
                );
            }
            let origin = Some(ModuleHookOrigin {
                name: alias.clone(),
                root: module.root.clone(),
                manifest: module.manifest.clone(),
                options_json: serde_json::to_vec(&projected)?,
            });
            let name = format!("@module/{alias}/{}", hook.name);
            let command = hook.command.clone();
            let rerun_on = hook
                .rerun_on
                .iter()
                .map(|path| resolve_rerun_path(module, alias, &hook.name, path))
                .collect::<Result<Vec<_>>>()?;
            match hook.stage {
                Stage::BeforeBuild => result.before_build.push(BeforeBuildHookConfig {
                    origin,
                    name,
                    command,
                    dev: hook.dev.unwrap_or(DevParticipation::Run),
                    rerun_on,
                    generates: hook
                        .generates
                        .iter()
                        .flatten()
                        .map(|path| {
                            let path = expand_path(path, alias);
                            relative(&path, "generates")?;
                            Ok(path)
                        })
                        .collect::<Result<_>>()?,
                    ..Default::default()
                }),
                Stage::GenerateOutputs => result.generate_outputs.push(OutputCommandConfig {
                    origin,
                    name,
                    command,
                    dev: hook.dev.unwrap_or(DevParticipation::Run),
                    rerun_on,
                    outputs: expand_outputs(hook.outputs.as_deref().unwrap_or_default(), alias)?,
                    ..Default::default()
                }),
                Stage::AfterPublish => result.after_publish.push(AfterPublishHookConfig {
                    origin,
                    name,
                    command,
                    dev: hook.dev.unwrap_or(DevParticipation::Skip),
                    rerun_on,
                    ..Default::default()
                }),
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_modules(
        site: &std::path::Path,
        declarations: &std::collections::BTreeMap<String, crate::modules::ModuleDeclaration>,
        mode: crate::inputs::InputMode,
    ) -> std::result::Result<
        std::collections::BTreeMap<String, crate::modules::ResolvedModule>,
        crate::modules::ModuleResolutionError,
    > {
        let roots =
            crate::inputs::module_roots(site, mode, None, declarations, &Default::default())?;
        crate::modules::resolve_modules_with_roots(site, declarations, &roots, mode)
    }

    fn parse(source: &str) -> std::result::Result<Vec<ModuleHook>, toml::de::Error> {
        #[derive(Deserialize)]
        struct Document {
            hooks: Vec<ModuleHook>,
        }
        toml::from_str::<Document>(source).map(|document| document.hooks)
    }

    #[test]
    fn module_hooks_validate_inactive_declarations() {
        for source in [
            "[[hooks]]\nstage = 'before-build'\nname = 'x'\ncommand = ['x']\nenabled-by = 'flag'\noutputs = []\n",
            "[[hooks]]\nstage = 'after-publish'\nname = 'x'\ncommand = ['x']\ngenerates = []\n",
            "[[hooks]]\nstage = 'generate-outputs'\nname = 'x'\ncommand = ['x']\noutputs = []\n",
            "[[hooks]]\nstage = 'before-build'\nname = 'x'\ncommand = ['x']\ndev = 'reuse'\n",
            "[[hooks]]\nstage = 'before-build'\nname = 'x'\ncommand = ['x']\nrerun-on = [{module = '../escape'}]\n",
        ] {
            assert!(
                validate_declarations(&parse(source).unwrap()).is_err(),
                "{source}"
            );
        }
        for source in [
            "[[hooks]]\nstage = 'mystery'\nname = 'x'\ncommand = ['x']\n",
            "[[hooks]]\nstage = 'before-build'\nname = 'x'\ncommand = ['x']\nextra = 1\n",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }

    mod module_hooks {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        use std::path::{Path, PathBuf};
        use std::sync::Arc;

        use crate::build::BuildRequest;
        use crate::cancellation::BuildCancellation;
        use crate::config::ResolvedSiteConfig;
        use crate::config::loading::{BuildOverrides, load_site_config};
        use crate::{BuildResources, BuildSession, InputScope, modules};

        fn write(root: &Path, path: &str, text: &str) {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn command() -> String {
            let exe = std::env::current_exe().unwrap();
            format!(
                "[{}, \"--exact\", \"modules::hooks::tests::module_hooks::module_hook_child\", \"--nocapture\"]",
                serde_json::to_string(&exe.to_string_lossy()).unwrap()
            )
        }

        fn describe(error: &anyhow::Error) -> String {
            format!(
                "{error:#}\nattached diagnostics: {:#?}",
                tola_diagnostic::attached(error)
            )
        }

        struct Site {
            _workspace: tempfile::TempDir,
            root: PathBuf,
            module: PathBuf,
        }
        impl Site {
            fn new(generates: &str, action: &str, extra: &str) -> Self {
                let workspace = tempfile::tempdir().unwrap();
                let root = workspace.path().join("project");
                let module = workspace.path().join("module");
                fs::create_dir_all(&root).unwrap();
                fs::create_dir_all(&module).unwrap();
                fs::create_dir_all(root.join("content")).unwrap();
                write(
                    &module,
                    "tola-module.toml",
                    &format!(
                        r#"
        name = "demo"
        version = "1.0.0"
        [[hooks]]
        stage = "before-build"
        name = "prepare"
        command = {}
        enabled-by = "enabled"
        options = ["enabled", "action", "dependency"]
        generates = [{generates}]
        [options]
        schema = "options.typ"
        export = "schema"
        [queries.nodes]
        entry = "query.typ"
        label = "nodes"
                "#,
                        command()
                    ),
                );
                write(&module, "dependency.txt", "original");
                write(
                    &module,
                    "options.typ",
                    &format!(
                        r#"#import "@tola/schema:0.0.0" as s
        #let schema = s.schema((
          enabled: s.optional(bool, default: true),
          action: s.optional(str, default: "{action}"),
          dependency: s.optional(str, default: read("dependency.txt")),
        ))
        "#
                    ),
                );
                write(&module, "query.typ", "#metadata((value: 1))<nodes>\n");
                write(
                    &root,
                    "site.typ",
                    "#document(\"index.html\", format: \"html\")[OK]\n",
                );
                write(
                    &root,
                    "tola.toml",
                    &format!(
                        r#"[modules.demo]
        path = "../module"
        [typst.fonts]
        system = false
        {extra}
        "#
                    ),
                );
                Self {
                    _workspace: workspace,
                    root,
                    module,
                }
            }
            fn config(&self) -> anyhow::Result<ResolvedSiteConfig> {
                Ok(load_site_config(
                    Some(&self.root.join("tola.toml")),
                    tola_typst::PackageLocations::default(),
                    &BuildOverrides::default(),
                )?
                .into_config())
            }
            fn session(&self) -> BuildSession {
                BuildSession::with_resources(
                    BuildResources::new().with_input_scope(InputScope::Offline),
                )
            }
            fn build(&self) -> anyhow::Result<()> {
                self.session()
                    .build_and_write(&self.config()?, BuildRequest::default())
                    .map(|_| ())
            }
            fn log(&self) -> String {
                fs::read_to_string(self.root.join("hook-log")).unwrap_or_default()
            }
        }

        fn disabled_site() -> Site {
            Site::new(
                "'generated/value.txt'",
                "normal",
                "[modules.demo.options]\nenabled = false\n",
            )
        }

        /// Executed as an independent hook child, never directly by the test harness.
        #[test]
        fn module_hook_child() {
            let Some(stage) = std::env::var_os("TOLA_HOOK_STAGE") else {
                return;
            };
            assert_eq!(stage, "before-build");
            let root = PathBuf::from(std::env::var_os("TOLA_PROJECT_ROOT").unwrap());
            assert_eq!(root, std::env::current_dir().unwrap());
            assert_eq!(
                PathBuf::from(std::env::var_os("TOLA_CONFIG_PATH").unwrap())
                    .canonicalize()
                    .unwrap(),
                root.join("tola.toml").canonicalize().unwrap()
            );
            assert_eq!(
                PathBuf::from(std::env::var_os("TOLA_CONTENT_ROOT").unwrap()),
                root.join("content")
            );
            let module = std::env::var_os("TOLA_MODULE_NAME");
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("hook-log"))
                .unwrap();
            match module {
                None => {
                    assert!(std::env::var_os("TOLA_MODULE_OPTIONS").is_none());
                    writeln!(log, "project").unwrap();
                }
                Some(name) => {
                    assert_eq!(name, "demo");
                    let module_root = PathBuf::from(std::env::var_os("TOLA_MODULE_ROOT").unwrap());
                    assert_eq!(
                        module_root,
                        root.parent()
                            .unwrap()
                            .join("module")
                            .canonicalize()
                            .unwrap()
                    );
                    assert_eq!(
                        PathBuf::from(std::env::var_os("TOLA_MODULE_MANIFEST").unwrap()),
                        module_root.join("tola-module.toml")
                    );
                    let options_path =
                        PathBuf::from(std::env::var_os("TOLA_MODULE_OPTIONS").unwrap());
                    assert!(options_path.starts_with(PathBuf::from(
                        std::env::var_os("TOLA_HOOK_TEMP_DIR").unwrap()
                    )));
                    let options: serde_json::Value =
                        serde_json::from_slice(&fs::read(options_path).unwrap()).unwrap();
                    assert_eq!(options["enabled"], true);
                    assert_eq!(options["dependency"], "original");
                    writeln!(log, "module:{}", options["action"].as_str().unwrap()).unwrap();
                    fs::create_dir_all(root.join("generated/fonts")).unwrap();
                    fs::write(root.join("generated/value.txt"), "generated").unwrap();
                    if options["action"] == "stale" {
                        fs::write(module_root.join("dependency.txt"), "changed").unwrap();
                    }
                }
            }
        }

        #[test]
        fn defaults_and_repeat_build() {
            let project_hook = format!(
                "[[build.hooks.before-build]]\nname = 'project'\ncommand = {}\n",
                command()
            );
            let site = Site::new("'generated/value.txt'", "normal", &project_hook);
            let config = modules::prepared::configure(
                &site.config().unwrap(),
                crate::inputs::InputMode::new(InputScope::Offline, false),
                &Default::default(),
            )
            .unwrap();
            let config = modules::prepared::configure(
                &config,
                crate::inputs::InputMode::new(InputScope::Offline, false),
                &Default::default(),
            )
            .unwrap();
            let mut session = site.session();
            session
                .build_and_write(&config, BuildRequest::default())
                .unwrap_or_else(|error| panic!("first build: {}", describe(&error)));
            session
                .build_and_write(&config, BuildRequest::default())
                .unwrap_or_else(|error| panic!("second build: {}", describe(&error)));
            assert_eq!(
                site.log(),
                "module:normal\nproject\nmodule:normal\nproject\n"
            );
            assert!(site.root.join("public/index.html").is_file());
        }

        #[test]
        fn stale_schema_input_aborts_build() {
            let site = Site::new("'generated/value.txt'", "stale", "");
            let error = site.build().expect_err("stale schema input must abort");
            assert!(
                format!("{error:#}").contains("changed after hook planning"),
                "{}",
                describe(&error),
            );
            assert_eq!(site.log(), "module:stale\n");
            assert_eq!(
                fs::read_to_string(site.module.join("dependency.txt")).unwrap(),
                "changed"
            );
            assert!(!site.root.join("public/index.html").exists());
        }

        #[test]
        fn query_reports_unverified_generation() {
            let site = disabled_site();
            let config = Arc::new(site.config().unwrap());
            let resolved = super::resolved_modules(
                config.get_root(),
                config.modules(),
                crate::inputs::InputMode::new(InputScope::Offline, false),
            )
            .unwrap();
            let module = &resolved["demo"];
            let result = modules::query::run(
                Arc::clone(&config),
                module,
                &module.queries["nodes"],
                "demo.nodes",
                BuildResources::new().with_input_scope(InputScope::Offline),
                vec![(site.root.join("site.typ"), Arc::from("Overlay content"))],
                &BuildCancellation::default(),
            )
            .unwrap();
            assert!(result.success, "{:?}", result.diagnostics);
            assert_eq!(result.input.hooks, "skipped");
            assert_eq!(result.input.mode, "overlay");
            assert_eq!(result.input.generation_status, "unverified");
            assert!(
                result
                    .input
                    .generated
                    .iter()
                    .any(|item| item.producer == "@module/demo/prepare"
                        && item.path == "generated/value.txt")
            );
            assert_eq!(site.log(), "");
        }
    }
}
