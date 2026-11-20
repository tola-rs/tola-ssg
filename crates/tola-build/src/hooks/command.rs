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
use std::sync::Arc;
use std::{
    ffi::{OsStr, OsString},
    process::Output,
};
use tola_input::layout;

pub(super) type ObserveFailure = Arc<dyn Fn() + Send + Sync>;

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
    fn context(&self, cache: &Path, temp: &Path) -> Result<crate::context::Context> {
        use crate::context::{Context, Hook, Module, Site};

        let mut context = Context::new(
            std::env::current_exe().context("locate the running Tola executable")?,
            Some(self.site.scope),
        );
        context.site = Some(Site {
            root: self.site.root.to_owned(),
            config_path: self.site.config.to_owned(),
            content_root: self.site.content.to_owned(),
        });
        if let Some(origin) = self.origin {
            context.module = Some(Module {
                name: origin.name.clone(),
                root: origin.root.clone(),
                manifest: origin.manifest.clone(),
                options: Some(serde_json::from_slice(&origin.options_json)?),
            });
        }
        context.hook = Some(Hook {
            name: self.declared_name().to_owned(),
            stage: self.directories.stage().as_str().to_owned(),
            mode: Some(self.mode.as_str().to_owned()),
            input_dir: self.directories.input().map(Path::to_owned),
            output_dir: self.directories.output().map(Path::to_owned),
            cache_dir: Some(cache.to_owned()),
            temp_dir: temp.to_owned(),
        });
        Ok(context)
    }

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
    pub context: crate::context::Context,
    pub temp_dir: &'a Path,
    pub cancellation: Option<&'a BuildCancellation>,
    label: &'a str,
    observe_failure: &'a ObserveFailure,
}

impl NuHook<'_> {
    /// Run the Nu executor with the same capture and cancellation as an argv hook.
    pub fn run_command(&self, command: &Command) -> Result<()> {
        run_observed_with(
            command,
            self.label,
            &|| {
                self.cancellation
                    .is_some_and(BuildCancellation::is_cancelled)
            },
            Some(self.observe_failure.as_ref()),
        )?;
        Ok(())
    }

    /// Stop dispatching this batch after observing a failure in the hook.
    pub fn observe_failure(&self) {
        (self.observe_failure)();
    }

    /// Notify the batch of a failure from this hook's report handler.
    pub fn failure_notifier(&self) -> Arc<dyn Fn() + Send + Sync> {
        Arc::clone(self.observe_failure)
    }
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

    pub(super) fn resources(&self) -> Option<&crate::BuildResources> {
        self.resources.as_ref()
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
    observe_failure: &ObserveFailure,
) -> Result<()> {
    if let Some(cancellation) = cancellation {
        cancellation.ensure_active()?;
    }
    match action.map_err(anyhow::Error::msg)? {
        Action::Argv(argv) => {
            let hook = hook_identity(call.directories.stage(), call.name);
            let invocation = HookInvocation::new(&argv, call)?;
            let completion = invocation.run(cancellation, observe_failure.as_ref());
            finish_report_with(
                &hook,
                invocation.reports,
                completion,
                diagnostics,
                observe_failure.as_ref(),
            )
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
                resources: runner.resources().cloned().unwrap_or_else(|| {
                    crate::BuildResources::new().with_input_scope(call.site.scope)
                }),
                config,
                path: &path,
                args: &args,
                scope: call.site.scope,
                context: call.context(&cache, temporary.path())?,
                temp_dir: temporary.path(),
                cancellation,
                label: call.name,
                observe_failure,
            });
            let reported = result.inspect_err(|error| {
                if !crate::cancellation::is_cancelled(error) {
                    observe_failure();
                }
            })?;
            tola_diagnostic::ensure_reports_succeed(&reported)
                .inspect_err(|_| observe_failure())?;
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

/// Expand the placeholders supplied to a site or module build hook's `argv`.
pub fn expand_hook_argument(
    argument: &str,
    root: &Path,
    origin: Option<&ModuleHookOrigin>,
    executable: &Path,
) -> Result<OsString> {
    let mut bindings = vec![
        ("{tola}", executable.as_os_str()),
        ("{project}", root.as_os_str()),
    ];
    if let Some(origin) = origin {
        bindings.extend([
            ("{module}", origin.root.as_os_str()),
            ("{name}", OsStr::new(&origin.name)),
        ]);
    } else {
        for marker in ["{module}", "{name}"] {
            anyhow::ensure!(
                !argument.contains(marker),
                "this hook has no module context for `{marker}`"
            );
        }
    }
    Ok(tola_module::expand_argument(argument, &bindings))
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
        let mut context = call.context(&cache, temporary.path())?;
        context.report_path = Some(reports.path().to_owned());
        let arguments = arguments
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                let argument = argument.as_ref();
                let expanded = match argument.to_str() {
                    Some(argument) => expand_hook_argument(
                        argument,
                        call.site.root,
                        call.origin,
                        &context.executable,
                    )
                    .with_context(|| format!("hook `{}` could not expand `argv`", call.name)),
                    None => Ok(argument.to_os_string()),
                }?;
                // Windows resolves explicit program paths before applying the child's cwd.
                if cfg!(windows)
                    && index == 0
                    && expanded
                        .as_encoded_bytes()
                        .iter()
                        .any(|byte| *byte == b'/' || *byte == b'\\')
                {
                    return Ok(call.site.root.join(expanded).into_os_string());
                }
                Ok(expanded)
            })
            .collect::<Result<Vec<_>>>()?;
        let command = context.command(
            Command::from_args(arguments)
                .cwd(call.site.root)
                .env("TMPDIR", temporary.path())
                .env("TMP", temporary.path())
                .env("TEMP", temporary.path()),
            temporary.path(),
        )?;
        Ok(Self {
            command,
            name: call.name,
            reports,
            _temporary: temporary,
        })
    }

    pub(super) fn run(
        &self,
        cancellation: Option<&BuildCancellation>,
        observe_failure: &dyn Fn(),
    ) -> Result<Output> {
        run_observed_with(
            &self.command,
            self.name,
            &|| cancellation.is_some_and(BuildCancellation::is_cancelled),
            Some(observe_failure),
        )
    }
}

pub(super) fn finish_report(
    hook: &str,
    reports: crate::report::ReportChannel,
    completion: Result<Output>,
    cancellation: Option<&BuildCancellation>,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
) -> Result<()> {
    finish_report_with(hook, reports, completion, diagnostics, &|| {})?;
    if let Some(cancellation) = cancellation {
        cancellation.ensure_active()?;
    }
    Ok(())
}

fn finish_report_with(
    hook: &str,
    reports: crate::report::ReportChannel,
    completion: Result<Output>,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
    observe_failure: &dyn Fn(),
) -> Result<()> {
    use crate::report::ReportError;
    use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};
    let cancelled = completion
        .as_ref()
        .is_err_and(crate::cancellation::is_cancelled);
    let report = reports.read().map_err(|error| {
        let message = format!("{hook} wrote a diagnostic report Tola could not read");
        let help = match &error {
            ReportError::Json(_) => {
                "Write a complete JSON report with `schemaVersion` and `diagnostics`"
            }
            ReportError::UnsupportedVersion(_) => "Set `schemaVersion` to `1`",
            ReportError::TooLarge { .. } => "Report fewer issues or shorten their messages",
            ReportError::UnknownSource(_) | ReportError::InvalidLocation { .. } => {
                "Use the source IDs and ranges supplied to this hook"
            }
            ReportError::InvalidDiagnostic { .. } => {
                "Correct the indicated item in the `diagnostics` array"
            }
            _ => "Check that the hook writes the report to the path Tola supplied",
        };
        let diagnostic = Diagnostic::new(
            tola_diagnostic::codes::build::HOOKS,
            Severity::Error,
            &message,
        )
        .with_note(error.to_string())
        .with_help(help);
        let error = anyhow::Error::new(error).context(message);
        anyhow::Error::new(DiagnosticError::attach(error, vec![diagnostic]))
    });
    let report = match report {
        Ok(report) => report,
        // A stopped command may leave only part of its report. Preserve a complete
        // report when there is one, but do not diagnose an interrupted write.
        Err(_) if cancelled => return completion.map(|_| ()),
        Err(error) => {
            observe_failure();
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
    tola_diagnostic::ensure_reports_succeed(&reported).inspect_err(|_| observe_failure())?;
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
    run_observed_with(command, hook, cancelled, None)
}

fn run_observed_with(
    command: &Command,
    hook: &str,
    cancelled: &impl Fn() -> bool,
    observe_failure: Option<&dyn Fn()>,
) -> Result<Output> {
    let mut observer = HookObserver::new(hook, observe_failure);
    let exit = match command.run(cancelled, &mut observer) {
        Ok(exit) => exit,
        Err(error) => {
            let error = uncompleted(command, error);
            return match observer.stop {
                Some(Stop::Exited(status)) if !status.success() => {
                    Err(anyhow::Error::new(CommandFailure::Exited {
                        status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    })
                    .context(error))
                }
                Some(Stop::Cancelled(_)) => {
                    Err(anyhow::Error::new(crate::cancellation::BuildCancelled).context(error))
                }
                _ => {
                    if let Some(observe_failure) = observe_failure {
                        observe_failure();
                    }
                    Err(error)
                }
            };
        }
    };
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
        let error = invocation
            .run(Some(&canceller.token()), &|| {})
            .unwrap_err();

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
                command
                    .env("TOLA_EXECUTABLE", &wrong)
                    .env(crate::context::PATH_ENV, &wrong);
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

        invocation.run(None, &|| {}).unwrap();
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
        let context = crate::build::tests::hook_context().unwrap();
        assert_eq!(context.input_scope.as_deref(), Some("pure"));
        assert_eq!(context.executable, std::env::current_exe().unwrap());
        assert_eq!(
            std::env::var_os("TOLA_EXECUTABLE").as_deref(),
            Some(context.executable.as_os_str())
        );
        let site = context.site.unwrap();
        assert_eq!(site.root, root);
        assert_eq!(site.config_path, root.join("custom/config.toml"));
        assert_eq!(site.content_root, root.join("custom/content"));
        let module = std::env::var("TOLA_HOOK_SCENE_MODULE").unwrap() == "yes";
        assert_eq!(context.module.is_some(), module);
        if let Some(module) = context.module {
            assert_eq!(module.name, "notes");
            assert_eq!(module.root, root.join("module"));
            assert_eq!(module.manifest, root.join("module/tola-module.toml"));
            assert_eq!(module.options.unwrap(), serde_json::json!({"search": true}));
        }
        assert_eq!(context.hook.unwrap().mode.as_deref(), Some(mode.as_str()));
        crate::build::tests::assert_hook_environment(&stage.to_string_lossy());
    }
}
