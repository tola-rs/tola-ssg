//! Hook command policy: what a configured command receives, and how it is run.

use anyhow::{Context, Result};
use tola_module::Action;
use tola_subprocess::{Command, Stop};

use super::hook_identity;
use super::report::{
    CommandFailure, HookObserver, InterruptionStep, cache_directory_error, check_exit,
    format_cancelled_output, render_capture, start_failure,
};

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::build::hooks::{HookStage, ModuleHookOrigin};
use crate::mode::BuildMode;
use std::fs;
use std::path::{Path, PathBuf};
use std::{ffi::OsStr, process::Output};
use tola_input::layout;

/// The directories one hook stage provides its command; a stage that provides none supplies none.
pub(super) enum HookDirectories<'a> {
    BeforeBuild,
    GenerateOutputs { input: &'a Path, output: &'a Path },
    AfterPublish { input: &'a Path },
}

impl<'a> HookDirectories<'a> {
    fn input(&self) -> Option<&'a Path> {
        match *self {
            Self::BeforeBuild => None,
            Self::GenerateOutputs { input, .. } | Self::AfterPublish { input } => Some(input),
        }
    }

    fn output(&self) -> Option<&'a Path> {
        match *self {
            Self::GenerateOutputs { output, .. } => Some(output),
            _ => None,
        }
    }

    fn stage(&self) -> HookStage {
        match self {
            Self::BeforeBuild => HookStage::BeforeBuild,
            Self::GenerateOutputs { .. } => HookStage::GenerateOutputs,
            Self::AfterPublish { .. } => HookStage::AfterPublish,
        }
    }
}

/// The host-resolved paths supplied to every hook.
pub(super) struct HookSitePaths<'a> {
    pub(super) root: &'a Path,
    pub(super) config: &'a Path,
    pub(super) content: &'a Path,
    pub(super) scope: tola_input::InputScope,
}

impl<'a> HookSitePaths<'a> {
    pub(super) fn new(config: &'a ResolvedSiteConfig, scope: tola_input::InputScope) -> Self {
        Self {
            root: config.get_root(),
            config: config.config_path(),
            content: &config.build().content_dir,
            scope,
        }
    }
}

/// One hook command's call: site, stage, name, mode, and stage directories.
pub(super) struct HookCall<'a> {
    pub(super) site: HookSitePaths<'a>,
    pub(super) name: &'a str,
    pub(super) origin: Option<&'a ModuleHookOrigin>,
    pub(super) mode: BuildMode,
    pub(super) directories: HookDirectories<'a>,
}

impl HookCall<'_> {
    fn declared_name(&self) -> &str {
        self.origin
            .and_then(|origin| self.name.strip_prefix(&format!("@module/{}/", origin.name)))
            .unwrap_or(self.name)
    }
}

/// A build hook invocation passed to the application's embedded Nu executor.
pub struct NuHook<'a> {
    pub modules: Option<&'a tola_module::prepared::PreparedModules>,
    pub assets: Option<&'a crate::asset::AssetUrls>,
    pub resources: crate::BuildResources,
    pub config: &'a ResolvedSiteConfig,
    pub path: &'a Path,
    pub args: &'a [String],
    pub scope: tola_input::InputScope,
    pub name: &'a str,
    pub origin: Option<&'a ModuleHookOrigin>,
    pub stage: HookStage,
    pub mode: BuildMode,
    pub input_dir: Option<&'a Path>,
    pub output_dir: Option<&'a Path>,
    pub cache_dir: &'a Path,
    pub temp_dir: &'a Path,
    pub cancellation: Option<&'a BuildCancellation>,
}

type RunNuHook =
    dyn for<'a> Fn(NuHook<'a>) -> Result<Vec<tola_diagnostic::Diagnostic>> + Send + Sync;

/// Application-owned runner for embedded Nu hooks.
#[derive(Clone)]
pub struct HookRunner {
    run: std::sync::Arc<RunNuHook>,
    modules: Option<std::sync::Arc<tola_module::prepared::PreparedModules>>,
    assets: Option<std::sync::Arc<crate::asset::AssetUrls>>,
    resources: Option<crate::BuildResources>,
}

impl HookRunner {
    pub fn new(
        run: impl for<'a> Fn(NuHook<'a>) -> Result<Vec<tola_diagnostic::Diagnostic>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            run: std::sync::Arc::new(run),
            modules: None,
            assets: None,
            resources: None,
        }
    }

    pub(crate) fn with_modules(mut self, modules: tola_module::prepared::PreparedModules) -> Self {
        self.modules = Some(std::sync::Arc::new(modules));
        self.assets = None;
        self
    }

    pub(crate) fn with_assets(mut self, assets: crate::asset::AssetUrls) -> Self {
        self.assets = Some(std::sync::Arc::new(assets));
        self
    }

    pub(crate) fn with_resources(mut self, resources: crate::BuildResources) -> Self {
        self.resources = Some(resources);
        self
    }

    pub(super) fn modules(&self) -> Option<&tola_module::prepared::PreparedModules> {
        self.modules.as_deref()
    }

    pub(super) fn assets(&self) -> Option<std::sync::Arc<crate::asset::AssetUrls>> {
        self.assets.clone()
    }
}

impl std::fmt::Debug for HookRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HookRunner")
    }
}

pub(super) fn run_action(
    config: &ResolvedSiteConfig,
    action: std::result::Result<Action, String>,
    call: HookCall<'_>,
    cancellation: Option<&BuildCancellation>,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
) -> Result<()> {
    if let Some(cancellation) = cancellation {
        cancellation.ensure_active()?;
    }
    match action.map_err(anyhow::Error::msg)? {
        Action::Argv(argv) => {
            let invocation = HookInvocation::new(&argv, call)?;
            let completion = invocation.run(cancellation);
            invocation.finish(completion, cancellation, diagnostics)
        }
        Action::Nu { path, args } => {
            let stage = call.directories.stage();
            let runner = config.hook_runner.as_ref().ok_or_else(|| {
                super::hook_contract_error(
                    format!(
                        "{} requires the embedded Nu runner",
                        hook_identity(stage, call.name)
                    ),
                    "run the build with Tola",
                )
            })?;
            let path = nu_script(call.site.root, call.origin, &path)?;
            let temporary = crate::filesystem::TemporaryDirectory::create(
                &layout::hook_scratch(call.site.root),
                "invocation",
            )?;
            let cache = prepare_cache(&call)?;
            let result = (runner.run)(NuHook {
                modules: runner.modules.as_deref(),
                assets: runner.assets.as_deref(),
                resources: runner.resources.clone().unwrap_or_else(|| {
                    crate::BuildResources::new().with_input_scope(call.site.scope)
                }),
                config,
                path: &path,
                args: &args,
                scope: call.site.scope,
                name: call.declared_name(),
                origin: call.origin,
                stage,
                mode: call.mode,
                input_dir: call.directories.input(),
                output_dir: call.directories.output(),
                cache_dir: &cache,
                temp_dir: temporary.path(),
                cancellation,
            });
            if let Some(cancellation) = cancellation {
                cancellation.ensure_active()?;
            }
            let reported = result?;
            tola_diagnostic::ensure_reports_succeed(&reported)?;
            diagnostics.extend(reported);
            Ok(())
        }
    }
}

pub(super) fn nu_script(
    site_root: &Path,
    origin: Option<&ModuleHookOrigin>,
    relative: &Path,
) -> Result<PathBuf> {
    let root = origin.map_or(site_root, |origin| origin.root.as_path());
    let path = root.join(relative);
    anyhow::ensure!(
        tola_input::normalize_existing_prefix(&path)
            .starts_with(tola_input::normalize_existing_prefix(root)),
        "the Nu script `{}` leaves its declaring directory",
        relative.display()
    );
    Ok(path)
}

pub(super) struct HookInvocation<'a> {
    command: Command,
    name: &'a str,
    reports: crate::report::ReportChannel,
    // Output inspection may still need scratch files after the child exits.
    _temporary: crate::filesystem::TemporaryDirectory,
}

impl<'a> HookInvocation<'a> {
    pub(super) fn new<S: AsRef<OsStr>>(arguments: &[S], call: HookCall<'a>) -> Result<Self> {
        let temporary = crate::filesystem::TemporaryDirectory::create(
            &layout::hook_scratch(call.site.root),
            "invocation",
        )?;
        let stage = call.directories.stage();
        let reports = crate::report::ReportChannel::new_in(
            temporary.path(),
            format!("hook:{}.{}", stage.as_str(), call.name),
            crate::report::ReportSources::new(),
        )?;
        let cache = prepare_cache(&call)?;
        let executable = std::env::current_exe().context("locate the running Tola executable")?;
        let mut bindings = vec![
            ("{tola}", executable.as_os_str()),
            ("{project}", call.site.root.as_os_str()),
        ];
        if let Some(origin) = call.origin {
            bindings.extend([
                ("{module}", origin.root.as_os_str()),
                ("{name}", OsStr::new(&origin.name)),
            ]);
        }
        let arguments = arguments
            .iter()
            .map(|argument| {
                let argument = argument.as_ref();
                match argument.to_str() {
                    Some(argument) => {
                        for marker in ["{module}", "{name}"] {
                            anyhow::ensure!(
                                call.origin.is_some() || !argument.contains(marker),
                                "hook `{}` has no module context for `{marker}`",
                                call.name
                            );
                        }
                        Ok(tola_module::expand_argument(argument, &bindings))
                    }
                    None => Ok(argument.to_os_string()),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let command = clear_tola_environment(Command::from_args(arguments))
            .cwd(call.site.root)
            .env("TOLA_EXECUTABLE", &executable)
            .env(crate::report::REPORT_PATH_ENV, reports.path())
            .env("TOLA_INPUT_SCOPE", call.site.scope.as_str())
            .env("TOLA_PROJECT_ROOT", call.site.root)
            .env("TOLA_CONFIG_PATH", call.site.config)
            .env("TOLA_CONTENT_ROOT", call.site.content)
            .env("TMPDIR", temporary.path())
            .env("TMP", temporary.path())
            .env("TEMP", temporary.path())
            .env("TOLA_HOOK_TEMP_DIR", temporary.path())
            .env("TOLA_HOOK_STAGE", stage.as_str())
            .env("TOLA_HOOK_NAME", call.declared_name())
            .env("TOLA_BUILD_MODE", call.mode.as_str())
            .env("TOLA_HOOK_CACHE_DIR", &cache);
        let command = match call.origin {
            Some(origin) => {
                let options = temporary.path().join("options.json");
                fs::write(&options, &origin.options_json).context("write module hook options")?;
                command
                    .env("TOLA_MODULE_NAME", &origin.name)
                    .env("TOLA_MODULE_ROOT", &origin.root)
                    .env("TOLA_MODULE_MANIFEST", &origin.manifest)
                    .env("TOLA_MODULE_OPTIONS", options)
            }
            None => command,
        };
        let command = match call.directories {
            HookDirectories::BeforeBuild => command,
            HookDirectories::GenerateOutputs { input, output } => command
                .env("TOLA_HOOK_INPUT_DIR", input)
                .env("TOLA_HOOK_OUTPUT_DIR", output),
            HookDirectories::AfterPublish { input } => command.env("TOLA_HOOK_INPUT_DIR", input),
        };
        Ok(Self {
            command,
            name: call.name,
            reports,
            _temporary: temporary,
        })
    }

    pub(super) fn finish(
        self,
        completion: Result<Output>,
        cancellation: Option<&BuildCancellation>,
        diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
    ) -> Result<()> {
        finish_report(self.reports, completion, cancellation, diagnostics)
    }

    pub(super) fn run(&self, cancellation: Option<&BuildCancellation>) -> Result<Output> {
        run_observed(&self.command, self.name, &|| {
            cancellation.is_some_and(BuildCancellation::is_cancelled)
        })
    }
}

/// Remove the parent invocation's values before assigning this hook's environment.
pub(super) fn clear_tola_environment(mut command: Command) -> Command {
    for name in [
        "TOLA_PROJECT_ROOT",
        "TOLA_CONFIG_PATH",
        "TOLA_CONTENT_ROOT",
        "TOLA_INPUT_SCOPE",
        "TOLA_BUILD_MODE",
        "TOLA_MODULE_NAME",
        "TOLA_MODULE_ROOT",
        "TOLA_MODULE_MANIFEST",
        "TOLA_MODULE_OPTIONS",
        "TOLA_HOOK_STAGE",
        "TOLA_HOOK_NAME",
        "TOLA_HOOK_INPUT_DIR",
        "TOLA_HOOK_OUTPUT_DIR",
        "TOLA_HOOK_TEMP_DIR",
        "TOLA_HOOK_CACHE_DIR",
        "TOLA_INIT_TARGET_DIR",
        "TOLA_INIT_OUTPUT_DIR",
        "TOLA_INIT_INTERACTIVE",
    ] {
        command = command.env_remove(name);
    }
    command
}

pub(super) fn finish_report(
    reports: crate::report::ReportChannel,
    completion: Result<Output>,
    cancellation: Option<&BuildCancellation>,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
) -> Result<()> {
    use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};
    if let Some(cancellation) = cancellation {
        cancellation.ensure_active()?;
    }
    let report = reports.read().map_err(|error| {
        let message = "invalid hook diagnostic report";
        let diagnostic = Diagnostic::new(
            tola_diagnostic::codes::build::HOOKS,
            Severity::Error,
            message,
        )
        .with_help("Check the report format and use only source locations supplied by Tola");
        let error = anyhow::Error::new(error).context(message);
        anyhow::Error::new(DiagnosticError::attach(error, vec![diagnostic]))
    });
    let report = match report {
        Ok(report) => report,
        Err(error) => {
            return match completion {
                Ok(_) => Err(error),
                Err(process_error) => {
                    let diagnostics = tola_diagnostic::attached(&error).unwrap().to_vec();
                    Err(DiagnosticError::attach(process_error.context(error), diagnostics).into())
                }
            };
        }
    };
    let reported = report.unwrap_or_default();
    if let Err(error) = completion {
        return if reported.is_empty() {
            Err(error)
        } else {
            Err(DiagnosticError::attach(error, reported).into())
        };
    }
    tola_diagnostic::ensure_reports_succeed(&reported)?;
    diagnostics.extend(reported);
    Ok(())
}

fn prepare_cache(call: &HookCall<'_>) -> Result<PathBuf> {
    let stage = call.directories.stage();
    let cache = hook_cache_directory(call.site.root, stage, call.name);
    fs::create_dir_all(&cache)
        .map_err(|cause| cache_directory_error(stage, &hook_identity(stage, call.name), cause))?;
    Ok(cache)
}

/// The persistent cache directory one hook identity owns.
fn hook_cache_directory(site_root: &Path, stage: HookStage, name: &str) -> PathBuf {
    layout::hook_cache(site_root)
        .join(stage.as_str())
        .join(tola_fs::encode_path_identity(Path::new(name)))
}

pub(super) fn run_observed(
    command: &Command,
    hook: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<Output> {
    let mut observer = HookObserver::new(hook);
    let exit = command
        .run(cancelled, &mut observer)
        .map_err(|error| uncompleted(command, error))?;
    let stdout = render_capture(&exit.stdout);
    let stderr = render_capture(&exit.stderr);
    match exit.stop {
        Stop::Exited(status) => check_exit(Output {
            status,
            stdout,
            stderr,
        }),
        Stop::Cancelled(_) => Err(anyhow::Error::new(crate::cancellation::BuildCancelled))
            .with_context(|| format_cancelled_output(hook, &stdout, &stderr)),
        _ => Err(anyhow::Error::new(CommandFailure::Interrupted {
            step: InterruptionStep::Run,
        })),
    }
}

fn uncompleted(command: &Command, error: tola_subprocess::Error) -> anyhow::Error {
    let program = command.program().to_string_lossy();
    let failure = match &error {
        tola_subprocess::Error::Spawn(source) => start_failure(&program, source.kind()),
        tola_subprocess::Error::Capture(_) => CommandFailure::Interrupted {
            step: InterruptionStep::Capture,
        },
        tola_subprocess::Error::Reader(_) | tola_subprocess::Error::Read(_) => {
            CommandFailure::Interrupted {
                step: InterruptionStep::Read,
            }
        }
        tola_subprocess::Error::Wait(_) => CommandFailure::Interrupted {
            step: InterruptionStep::Wait,
        },
        _ => CommandFailure::Interrupted {
            step: InterruptionStep::Run,
        },
    };
    anyhow::Error::new(failure).context(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as ProcessCommand;

    /// The interface names Tola sets for a hook command; a scene inherits wrong values under
    /// exactly these names, so every value its child reads must be one Tola wrote.
    const INTERFACE_VARIABLES: [&str; 18] = [
        "TOLA_EXECUTABLE",
        "TOLA_INPUT_SCOPE",
        "TOLA_MODULE_NAME",
        "TOLA_MODULE_ROOT",
        "TOLA_MODULE_MANIFEST",
        "TOLA_MODULE_OPTIONS",
        "TOLA_PROJECT_ROOT",
        "TOLA_CONFIG_PATH",
        "TOLA_CONTENT_ROOT",
        "TOLA_HOOK_STAGE",
        "TOLA_BUILD_MODE",
        "TOLA_HOOK_CACHE_DIR",
        "TOLA_HOOK_TEMP_DIR",
        "TOLA_HOOK_INPUT_DIR",
        "TOLA_HOOK_OUTPUT_DIR",
        "TMPDIR",
        "TMP",
        "TEMP",
    ];

    #[test]
    fn cancelled_command_keeps_typed_error() {
        let directory = tempfile::tempdir().unwrap();
        let canceller = crate::cancellation::BuildCanceller::new();
        canceller.cancel();
        let invocation = HookInvocation::new(
            &["tola-hook-missing-program"],
            HookCall {
                site: HookSitePaths {
                    root: directory.path(),
                    config: directory.path(),
                    content: directory.path(),
                    scope: tola_input::InputScope::Online,
                },
                name: "search",
                origin: None,
                mode: BuildMode::Production,
                directories: HookDirectories::BeforeBuild,
            },
        )
        .unwrap();
        let error = invocation.run(Some(&canceller.token())).unwrap_err();

        assert!(error.is::<crate::cancellation::BuildCancelled>());
    }

    /// Every stage's child receives the environment Tola built, even when the process
    /// running Tola sets wrong values under the same names.
    #[test]
    fn hook_environment_replaces_inherited_values() {
        let directory = tempfile::tempdir().unwrap();
        let wrong = directory.path().join("wrong");
        for (stage, mode) in [
            ("before-build", "dev"),
            ("before-build", "prod"),
            ("generate-outputs", "dev"),
            ("generate-outputs", "prod"),
            ("after-publish", "dev"),
            ("after-publish", "prod"),
        ] {
            for module in [false, true] {
                let mut command = ProcessCommand::new(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "hooks::command::tests::hook_environment_scene",
                        "--nocapture",
                    ])
                    .env("TOLA_HOOK_SCENE_ROOT", directory.path())
                    .env("TOLA_HOOK_SCENE_STAGE", stage)
                    .env("TOLA_HOOK_SCENE_MODE", mode)
                    .env("TOLA_HOOK_SCENE_MODULE", if module { "yes" } else { "no" });
                for variable in INTERFACE_VARIABLES {
                    command.env(variable, wrong.join(variable));
                }
                let output = command.output().unwrap();
                assert!(
                    output.status.success(),
                    "{stage} in {mode}:\n{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }

    /// Runs one stage of the environment contract inside a process whose own environment
    /// sets wrong interface values, so its child asserts what Tola replaced.
    #[test]
    fn hook_environment_scene() {
        let Some(root) = std::env::var_os("TOLA_HOOK_SCENE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let config = root.join("custom/config.toml");
        let content = root.join("custom/content");
        let origin = ModuleHookOrigin {
            name: "notes".into(),
            root: root.join("module"),
            manifest: root.join("module/tola-module.toml"),
            options_json: br#"{"search":true}"#.to_vec(),
        };
        let is_module = std::env::var("TOLA_HOOK_SCENE_MODULE").unwrap() == "yes";
        let stage = std::env::var("TOLA_HOOK_SCENE_STAGE").unwrap();
        let mode = std::env::var("TOLA_HOOK_SCENE_MODE").unwrap();
        let input = root.join("input");
        let output = root.join("output");
        for directory in [&input, &output] {
            fs::create_dir_all(directory).unwrap();
        }
        let directories = match stage.as_str() {
            "before-build" => HookDirectories::BeforeBuild,
            "generate-outputs" => HookDirectories::GenerateOutputs {
                input: &input,
                output: &output,
            },
            "after-publish" => HookDirectories::AfterPublish { input: &input },
            other => panic!("unknown hook stage `{other}`"),
        };
        let mode = match mode.as_str() {
            "dev" => BuildMode::Development,
            "prod" => BuildMode::Production,
            other => panic!("unknown build mode `{other}`"),
        };
        let invocation = HookInvocation::new(
            &crate::build::tests::hook_child_command(
                "hooks::command::tests::child_hook_environment",
            ),
            HookCall {
                site: HookSitePaths {
                    root: &root,
                    config: &config,
                    content: &content,
                    scope: tola_input::InputScope::Pure,
                },
                name: "environment",
                origin: is_module.then_some(&origin),
                mode,
                directories,
            },
        )
        .unwrap();

        invocation.run(None).unwrap();
    }

    /// Asserts the environment Tola built for it, against the expectations the scene
    /// inherited as ordinary environment values.
    #[test]
    fn child_hook_environment() {
        let Some(stage) = std::env::var_os("TOLA_HOOK_SCENE_STAGE") else {
            return;
        };
        let mode = std::env::var("TOLA_HOOK_SCENE_MODE").unwrap();
        let root = PathBuf::from(std::env::var_os("TOLA_HOOK_SCENE_ROOT").unwrap());
        assert_eq!(std::env::var("TOLA_INPUT_SCOPE").unwrap(), "pure");
        assert_eq!(
            std::env::var_os("TOLA_EXECUTABLE"),
            Some(std::env::current_exe().unwrap().into_os_string())
        );
        for (variable, expected) in [
            ("TOLA_PROJECT_ROOT", root.clone()),
            ("TOLA_CONFIG_PATH", root.join("custom/config.toml")),
            ("TOLA_CONTENT_ROOT", root.join("custom/content")),
        ] {
            assert_eq!(std::env::var_os(variable), Some(expected.into_os_string()));
        }
        let module = std::env::var("TOLA_HOOK_SCENE_MODULE").unwrap() == "yes";
        for (variable, expected) in [
            ("TOLA_MODULE_NAME", std::ffi::OsString::from("notes")),
            ("TOLA_MODULE_ROOT", root.join("module").into_os_string()),
            (
                "TOLA_MODULE_MANIFEST",
                root.join("module/tola-module.toml").into_os_string(),
            ),
        ] {
            assert_eq!(std::env::var_os(variable), module.then_some(expected));
        }
        match std::env::var_os("TOLA_MODULE_OPTIONS") {
            Some(path) if module => {
                assert_eq!(std::fs::read(path).unwrap(), br#"{"search":true}"#);
            }
            None if !module => {}
            value => panic!("unexpected module options path: {value:?}"),
        }

        assert_eq!(
            std::env::var_os("TOLA_BUILD_MODE").as_deref(),
            Some(std::ffi::OsStr::new(&mode))
        );
        crate::build::tests::assert_hook_environment(&stage.to_string_lossy());
    }
}
