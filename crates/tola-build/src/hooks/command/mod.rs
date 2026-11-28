//! Hook command policy: what a configured command receives, and how it is run.

#[cfg(test)]
mod tests;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use anyhow::{Context, Result};
use tola_input::{FilePatterns, FileSelection, layout};
use tola_module::hooks::RerunOn;
use tola_module::{Action, CommandOutput};
use tola_subprocess::{Command, Stop};

use super::hook_identity;
use super::report::{
    CommandFailure, HookObserver, InterruptionStep, cache_directory_error, check_exit,
    format_cancelled_output, render_capture, start_failure,
};
use crate::cancellation::BuildCancellation;
use crate::config::SiteConfig;
use crate::config::section::build::hooks::{HookStage, ModuleHookOrigin};
use crate::mode::BuildMode;

pub(super) type ObserveFailure = Arc<dyn Fn() + Send + Sync>;

pub(super) enum HookDirectories<'a> {
    BeforeBuild,
    GenerateOutputs {
        input: &'a Path,
        output: &'a Path,
        outputs: &'a [CommandOutput],
    },
    AfterPublish {
        input: &'a Path,
    },
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

pub(super) struct HookSitePaths<'a> {
    pub(super) root: &'a Path,
    pub(super) config: &'a Path,
    pub(super) content: &'a Path,
    pub(super) scope: tola_input::InputScope,
}

impl<'a> HookSitePaths<'a> {
    pub(super) fn new(cfg: &'a SiteConfig, scope: tola_input::InputScope) -> Self {
        Self {
            root: cfg.root(),
            config: cfg.config_path(),
            content: &cfg.build().content_dir,
            scope,
        }
    }
}

pub(super) struct HookCall<'a> {
    pub(super) site: HookSitePaths<'a>,
    pub(super) name: &'a str,
    pub(super) origin: Option<&'a ModuleHookOrigin>,
    pub(super) mode: BuildMode,
    pub(super) directories: HookDirectories<'a>,
}

impl HookCall<'_> {
    fn context(&self, cache: &Path, temp: &Path) -> Result<crate::context::Context> {
        use crate::context::{Context, Hook, Module, Owner, Site};

        let outputs = match self.directories {
            HookDirectories::GenerateOutputs { outputs, .. } => outputs.to_vec(),
            _ => Vec::new(),
        };
        let mut ctx = Context::new(
            std::env::current_exe().context("locate the running Tola executable")?,
            Some(self.site.scope),
        );
        ctx.site = Some(Site {
            root: self.site.root.to_owned(),
            config_path: self.site.config.to_owned(),
            content_root: self.site.content.to_owned(),
        });
        if let Some(origin) = self.origin {
            ctx.module = Some(Module {
                name: origin.name.as_str().to_owned(),
                root: origin.root.clone(),
                manifest: origin.manifest.clone(),
                options: Some(serde_json::from_slice(&origin.options_json)?),
            });
            ctx.owner = Some(Owner {
                root: origin.owner.root(self.site.root).to_owned(),
                module: origin.owner.module_name().map(str::to_owned),
            });
        }
        ctx.hook = Some(Hook {
            name: self.declared_name().to_owned(),
            stage: self.directories.stage().as_str().to_owned(),
            mode: Some(self.mode.as_str().to_owned()),
            input_dir: self.directories.input().map(Path::to_owned),
            output_dir: self.directories.output().map(Path::to_owned),
            outputs,
            cache_dir: Some(cache.to_owned()),
            temp_dir: temp.to_owned(),
        });
        Ok(ctx)
    }

    fn declared_name(&self) -> &str {
        self.origin
            .and_then(|o| self.name.strip_prefix(&format!("@module/{}/", o.name)))
            .unwrap_or(self.name)
    }
}

/// A build hook invocation passed to the application's embedded Nu executor.
pub struct NuHook<'a> {
    pub modules: Option<&'a tola_module::bundle::ModuleBundle>,
    pub assets: Option<&'a crate::asset::AssetUrls>,
    pub resources: crate::BuildResources,
    pub config: &'a SiteConfig,
    pub path: &'a Path,
    pub args: &'a [String],
    pub scope: tola_input::InputScope,
    pub context: crate::context::Context,
    pub temp_dir: &'a Path,
    pub cancellation: Option<&'a BuildCancellation>,
    origin: Option<&'a ModuleHookOrigin>,
    rerun_on: &'a RerunOn,
    configured_assets: Option<&'a crate::asset::SiteAssets>,
    label: &'a str,
    observe_failure: &'a ObserveFailure,
}

impl NuHook<'_> {
    /// Read a digest of the hook's declared rerun-on files on demand.
    pub fn rerun_on_digest(&self) -> impl Fn() -> Result<Option<String>> + Send + Sync + use<> {
        let root = self.config.root().to_owned();
        let origin = self.origin.cloned();
        let rerun_on = self.rerun_on.clone();
        let cancel = self.cancellation.cloned().unwrap_or_default();
        move || super::reuse::rerun_on_digest(&root, origin.as_ref(), &rerun_on, &cancel)
    }

    /// Select configured assets by their source paths after asset discovery.
    pub fn asset_reader(
        &self,
    ) -> Option<
        impl Fn(Option<String>, FilePatterns) -> Result<serde_json::Value> + Send + Sync + use<>,
    > {
        let assets = self.configured_assets?.clone();
        let cfg = self.config.clone();
        let cancel = self.cancellation.cloned().unwrap_or_default();
        Some(move |module: Option<String>, patterns| {
            if let Some(alias) = &module {
                anyhow::ensure!(
                    cfg.modules().contains_key(alias),
                    "no configured module is named `{alias}`"
                );
            }
            let selection = FileSelection::new(&patterns)?;
            assets.select(cfg.root(), module.as_deref(), &selection, &cancel)
        })
    }

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
    run: Arc<RunNuHook>,
    modules: Option<Arc<tola_module::bundle::ModuleBundle>>,
    assets: Option<Arc<crate::asset::AssetUrls>>,
    configured_assets: Option<crate::asset::SiteAssets>,
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
            run: Arc::new(run),
            modules: None,
            assets: None,
            configured_assets: None,
            resources: None,
        }
    }

    pub(crate) fn with_modules(mut self, modules: tola_module::bundle::ModuleBundle) -> Self {
        self.modules = Some(Arc::new(modules));
        self.assets = None;
        self.configured_assets = None;
        self
    }

    pub(crate) fn with_assets(
        mut self,
        assets: crate::asset::SiteAssets,
        config: &SiteConfig,
    ) -> Result<Self> {
        self.assets = Some(Arc::new(assets.asset_urls(config)?));
        self.configured_assets = Some(assets);
        Ok(self)
    }

    pub(crate) fn with_resources(mut self, resources: crate::BuildResources) -> Self {
        self.resources = Some(resources);
        self
    }

    pub(super) fn modules(&self) -> Option<&tola_module::bundle::ModuleBundle> {
        self.modules.as_deref()
    }

    pub(super) fn assets(&self) -> Option<Arc<crate::asset::AssetUrls>> {
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
    cfg: &SiteConfig,
    action: std::result::Result<Action, String>,
    call: HookCall<'_>,
    rerun_on: &RerunOn,
    cancel: Option<&BuildCancellation>,
    diags: &mut Vec<tola_diagnostic::Diagnostic>,
    observe_failure: &ObserveFailure,
) -> Result<()> {
    if let Some(cancel) = cancel {
        cancel.ensure_active()?;
    }
    match action.map_err(anyhow::Error::msg)? {
        Action::Argv(argv) => {
            let hook = hook_identity(call.directories.stage(), call.name);
            let invocation = HookInvocation::new(&argv, call)?;
            let completion = invocation.run(cancel, observe_failure.as_ref());
            finish_report_with(
                &hook,
                invocation.reports,
                completion,
                diags,
                observe_failure.as_ref(),
            )
        }
        Action::Nu { path, args } => {
            let stage = call.directories.stage();
            let runner = cfg.hook_runner.as_ref().ok_or_else(|| {
                super::hook_contract_error(
                    format!(
                        "{} requires the embedded Nu runner",
                        hook_identity(stage, call.name)
                    ),
                    "run the build with Tola",
                )
            })?;
            let path = nu_script(call.site.root, call.origin, &path)?;
            let tmp = crate::filesystem::TemporaryDirectory::create(
                &layout::hook_scratch(call.site.root),
                "invocation",
            )?;
            let cache = prepare_cache(&call)?;
            let res = (runner.run)(NuHook {
                modules: runner.modules.as_deref(),
                assets: runner.assets.as_deref(),
                resources: runner.resources().cloned().unwrap_or_else(|| {
                    crate::BuildResources::new().with_input_scope(call.site.scope)
                }),
                config: cfg,
                path: &path,
                args: &args,
                scope: call.site.scope,
                context: call.context(&cache, tmp.path())?,
                temp_dir: tmp.path(),
                cancellation: cancel,
                origin: call.origin,
                rerun_on,
                configured_assets: runner.configured_assets.as_ref(),
                label: call.name,
                observe_failure,
            });
            let reported = res.inspect_err(|err| {
                if !crate::cancellation::is_cancelled(err) {
                    observe_failure();
                }
            })?;
            tola_diagnostic::ensure_reports_succeed(&reported)
                .inspect_err(|_| observe_failure())?;
            diags.extend(reported);
            Ok(())
        }
    }
}

pub(super) fn nu_script(
    site_root: &Path,
    origin: Option<&ModuleHookOrigin>,
    rel: &Path,
) -> Result<PathBuf> {
    let root = origin.map_or(site_root, |o| o.root.as_path());
    let path = root.join(rel);
    anyhow::ensure!(
        tola_input::normalize_existing_prefix(&path)
            .starts_with(tola_input::normalize_existing_prefix(root)),
        "the Nu script `{}` leaves its declaring directory",
        rel.display()
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
            ("{name}", OsStr::new(origin.name.as_str())),
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
    _tmp: crate::filesystem::TemporaryDirectory,
}

impl<'a> HookInvocation<'a> {
    pub(super) fn new<S: AsRef<OsStr>>(args: &[S], call: HookCall<'a>) -> Result<Self> {
        let tmp = crate::filesystem::TemporaryDirectory::create(
            &layout::hook_scratch(call.site.root),
            "invocation",
        )?;
        let stage = call.directories.stage();
        let reports = crate::report::ReportChannel::new_in(
            tmp.path(),
            format!("hook:{}.{}", stage.as_str(), call.name),
            crate::report::ReportSources::new(),
        )?;
        let cache = prepare_cache(&call)?;
        let mut ctx = call.context(&cache, tmp.path())?;
        ctx.report_path = Some(reports.path().to_owned());
        let args = args
            .iter()
            .enumerate()
            .map(|(idx, arg)| {
                let arg = arg.as_ref();
                let expanded = match arg.to_str() {
                    Some(arg) => {
                        expand_hook_argument(arg, call.site.root, call.origin, &ctx.executable)
                            .with_context(|| {
                                format!("hook `{}` could not expand `argv`", call.name)
                            })
                    }
                    None => Ok(arg.to_os_string()),
                }?;
                // Windows resolves explicit program paths before applying the child's cwd.
                if cfg!(windows)
                    && idx == 0
                    && expanded
                        .as_encoded_bytes()
                        .iter()
                        .any(|b| *b == b'/' || *b == b'\\')
                {
                    return Ok(call.site.root.join(expanded).into_os_string());
                }
                Ok(expanded)
            })
            .collect::<Result<Vec<_>>>()?;
        let command = ctx.command(
            Command::from_args(args)
                .cwd(call.site.root)
                .env("TMPDIR", tmp.path())
                .env("TMP", tmp.path())
                .env("TEMP", tmp.path()),
            tmp.path(),
        )?;
        Ok(Self {
            command,
            name: call.name,
            reports,
            _tmp: tmp,
        })
    }

    pub(super) fn run(
        &self,
        cancel: Option<&BuildCancellation>,
        observe_failure: &dyn Fn(),
    ) -> Result<Output> {
        run_observed_with(
            &self.command,
            self.name,
            &|| cancel.is_some_and(BuildCancellation::is_cancelled),
            Some(observe_failure),
        )
    }
}

pub(super) fn finish_report(
    hook: &str,
    reports: crate::report::ReportChannel,
    completion: Result<Output>,
    cancel: Option<&BuildCancellation>,
    diags: &mut Vec<tola_diagnostic::Diagnostic>,
) -> Result<()> {
    finish_report_with(hook, reports, completion, diags, &|| {})?;
    if let Some(cancel) = cancel {
        cancel.ensure_active()?;
    }
    Ok(())
}

fn finish_report_with(
    hook: &str,
    reports: crate::report::ReportChannel,
    completion: Result<Output>,
    diags: &mut Vec<tola_diagnostic::Diagnostic>,
    observe_failure: &dyn Fn(),
) -> Result<()> {
    use crate::report::ReportError;
    use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};

    let cancelled = completion
        .as_ref()
        .is_err_and(crate::cancellation::is_cancelled);
    let report = reports.read().map_err(|err| {
        let msg = format!("{hook} wrote a diagnostic report Tola could not read");
        let help = match &err {
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
        let diag = Diagnostic::new(tola_diagnostic::codes::build::HOOKS, Severity::Error, &msg)
            .with_note(err.to_string())
            .with_help(help);
        let err = anyhow::Error::new(err).context(msg);
        anyhow::Error::new(DiagnosticError::attach(err, vec![diag]))
    });
    let report = match report {
        Ok(report) => report,
        // Preserve a complete report when cancelled, but don't diagnose an interrupted write.
        Err(_) if cancelled => return completion.map(|_| ()),
        Err(err) => {
            observe_failure();
            return match completion {
                Ok(_) => Err(err),
                Err(proc_err) => {
                    let attached = tola_diagnostic::attached(&err).unwrap().to_vec();
                    Err(DiagnosticError::attach(proc_err.context(err), attached).into())
                }
            };
        }
    };
    let reported = report.unwrap_or_default();
    if let Err(err) = completion {
        return if reported.is_empty() {
            Err(err)
        } else {
            Err(DiagnosticError::attach(err, reported).into())
        };
    }
    tola_diagnostic::ensure_reports_succeed(&reported).inspect_err(|_| observe_failure())?;
    diags.extend(reported);
    Ok(())
}

fn prepare_cache(call: &HookCall<'_>) -> Result<PathBuf> {
    let stage = call.directories.stage();
    let cache = layout::hook_cache(call.site.root)
        .join(stage.as_str())
        .join(tola_fs::encode_path_identity(Path::new(call.name)));
    fs::create_dir_all(&cache)
        .map_err(|cause| cache_directory_error(stage, &hook_identity(stage, call.name), cause))?;
    Ok(cache)
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
        Err(err) => {
            let err = uncompleted(command, err);
            return match observer.stop {
                Some(Stop::Exited(status)) if !status.success() => {
                    Err(anyhow::Error::new(CommandFailure::Exited {
                        status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    })
                    .context(err))
                }
                Some(Stop::Cancelled(_)) => {
                    Err(anyhow::Error::new(crate::cancellation::BuildCancelled).context(err))
                }
                _ => {
                    if let Some(observe_failure) = observe_failure {
                        observe_failure();
                    }
                    Err(err)
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

fn uncompleted(command: &Command, err: tola_subprocess::Error) -> anyhow::Error {
    let program = command.program().to_string_lossy();
    let failure = match &err {
        tola_subprocess::Error::Spawn(src) => start_failure(&program, src.kind()),
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
    anyhow::Error::new(failure).context(err)
}
