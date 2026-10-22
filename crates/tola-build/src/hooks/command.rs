//! Hook command policy: what a configured command receives, and how it is run.

use anyhow::{Context, Result};
use tola_subprocess::{Command, Stop};

use super::hook_identity;
use super::report::{
    CommandFailure, HookObserver, InterruptionStep, cache_directory_error, check_exit,
    format_cancelled_output, render_capture, start_failure,
};

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::build::hooks::{HookStage, ModuleHookOrigin};
use crate::filesystem::layout;
use crate::mode::BuildMode;
use std::fs;
use std::path::{Path, PathBuf};
use std::{ffi::OsStr, process::Output};

/// The directories one hook stage provides its command; a stage that provides none supplies none.
pub(super) enum HookDirectories<'a> {
    BeforeBuild,
    GenerateOutputs { input: &'a Path, output: &'a Path },
    AfterPublish { input: &'a Path },
}

impl HookDirectories<'_> {
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
    pub(super) scope: crate::InputScope,
}

impl<'a> From<&'a ResolvedSiteConfig> for HookSitePaths<'a> {
    fn from(config: &'a ResolvedSiteConfig) -> Self {
        Self {
            root: config.get_root(),
            config: config.config_path(),
            content: &config.build().content_dir,
            scope: config.module_inputs.scope,
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
        let cache = hook_cache_directory(call.site.root, stage, call.name);
        // The cache outlives this invocation, so the temporary directory never removes it.
        fs::create_dir_all(&cache).map_err(|cause| {
            cache_directory_error(stage, &hook_identity(stage, call.name), cause)
        })?;
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
        let arguments = arguments.iter().map(|argument| {
            let argument = argument.as_ref();
            match argument.to_str() {
                Some(argument) => crate::modules::expand_argument(argument, &bindings),
                None => argument.to_os_string(),
            }
        });
        let command = Command::from_args(arguments)
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
            None => command
                .env_remove("TOLA_MODULE_NAME")
                .env_remove("TOLA_MODULE_ROOT")
                .env_remove("TOLA_MODULE_MANIFEST")
                .env_remove("TOLA_MODULE_OPTIONS"),
        };
        let command = match call.directories {
            HookDirectories::BeforeBuild => command
                .env_remove("TOLA_HOOK_INPUT_DIR")
                .env_remove("TOLA_HOOK_OUTPUT_DIR"),
            HookDirectories::GenerateOutputs { input, output } => command
                .env("TOLA_HOOK_INPUT_DIR", input)
                .env("TOLA_HOOK_OUTPUT_DIR", output),
            HookDirectories::AfterPublish { input } => command
                .env("TOLA_HOOK_INPUT_DIR", input)
                .env_remove("TOLA_HOOK_OUTPUT_DIR"),
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
    ) -> Result<bool> {
        use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};
        if let Some(cancellation) = cancellation {
            cancellation.ensure_active()?;
        }
        let report = self.reports.read().map_err(|error| {
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
                        Err(
                            DiagnosticError::attach(process_error.context(error), diagnostics)
                                .into(),
                        )
                    }
                };
            }
        };
        let submitted = report.is_some();
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
        Ok(submitted)
    }

    pub(super) fn run(&self, cancellation: Option<&BuildCancellation>) -> Result<Output> {
        run_observed(&self.command, self.name, &|| {
            cancellation.is_some_and(BuildCancellation::is_cancelled)
        })
    }
}

/// The persistent cache directory one hook identity owns.
fn hook_cache_directory(site_root: &Path, stage: HookStage, name: &str) -> PathBuf {
    layout::hook_cache(site_root)
        .join(stage.as_str())
        .join(crate::filesystem::encode_path_identity(Path::new(name)))
}

fn run_observed(command: &Command, hook: &str, cancelled: &impl Fn() -> bool) -> Result<Output> {
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
                    scope: crate::InputScope::Online,
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
                    scope: crate::InputScope::Pure,
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
