//! Hook command policy: what a configured command receives, and how it is run.
//!
//! Process execution, process containment, bounded capture, and termination belong
//! to `tola-subprocess`. This module decides which environment a configured hook
//! runs with and turns a command that did not complete into the failure its stage
//! reports to the site author.

use anyhow::{Context, Result};
use tola_subprocess::{Command, Stop};

use super::hook_identity;
use super::report::{
    CommandFailure, HookObserver, InterruptionStep, cache_directory_error, check_exit,
    format_cancelled_output, render_capture, start_failure,
};

use crate::cancellation::BuildCancellation;
use crate::config::section::build::hooks::HookStage;
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

/// One configured hook command's call: the site it runs in, its stage, its configured name,
/// the build mode, and the directories the stage provides.
pub(super) struct HookCall<'a> {
    pub(super) site_root: &'a Path,
    pub(super) name: &'a str,
    pub(super) mode: BuildMode,
    pub(super) directories: HookDirectories<'a>,
}

pub(super) struct HookInvocation<'a> {
    command: Command,
    name: &'a str,
    // Output inspection may still need scratch files after the child exits.
    _temporary: crate::filesystem::TemporaryDirectory,
}

impl<'a> HookInvocation<'a> {
    pub(super) fn new<S: AsRef<OsStr>>(arguments: &[S], call: HookCall<'a>) -> Result<Self> {
        let temporary = crate::filesystem::TemporaryDirectory::create(
            &call
                .site_root
                .join(crate::filesystem::INTERNAL_DIR)
                .join("hook-tmp"),
            "invocation",
        )?;
        let stage = call.directories.stage();
        let cache = hook_cache_directory(call.site_root, stage, call.name);
        // The cache outlives this invocation, so it is created here and never removed by
        // the temporary directory's owner.
        fs::create_dir_all(&cache).map_err(|cause| {
            cache_directory_error(stage, &hook_identity(stage, call.name), cause)
        })?;
        let command = Command::from_args(arguments.iter().map(AsRef::as_ref))
            .cwd(call.site_root)
            .env("TMPDIR", temporary.path())
            .env("TMP", temporary.path())
            .env("TEMP", temporary.path())
            .env("TOLA_HOOK_TEMP_DIR", temporary.path())
            .env("TOLA_HOOK_STAGE", stage.as_str())
            .env("TOLA_BUILD_MODE", call.mode.as_str())
            .env("TOLA_HOOK_CACHE_DIR", &cache);
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
            _temporary: temporary,
        })
    }

    pub(super) fn run(&self, cancellation: Option<&BuildCancellation>) -> Result<Output> {
        run_observed(&self.command, self.name, &|| {
            cancellation.is_some_and(BuildCancellation::is_cancelled)
        })
    }
}

/// The persistent cache directory one hook identity owns, below the site's internal directory.
///
/// A configured name is one user-written word that may still contain path separators, `..`, or
/// non-ASCII characters, so it is hex-encoded into a single component: distinct names never
/// collide, and no name can reach outside the site root.
fn hook_cache_directory(site_root: &Path, stage: HookStage, name: &str) -> PathBuf {
    site_root
        .join(crate::filesystem::INTERNAL_DIR)
        .join("hook-cache")
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
    const INTERFACE_VARIABLES: [&str; 9] = [
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
                site_root: directory.path(),
                name: "search",
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
            let mut command = ProcessCommand::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "hooks::command::tests::hook_environment_scene",
                    "--nocapture",
                ])
                .env("TOLA_HOOK_SCENE_ROOT", directory.path())
                .env("TOLA_HOOK_SCENE_STAGE", stage)
                .env("TOLA_HOOK_SCENE_MODE", mode);
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

    /// Runs one stage of the environment contract inside a process whose own environment
    /// sets wrong interface values, so its child asserts what Tola replaced.
    #[test]
    fn hook_environment_scene() {
        let Some(root) = std::env::var_os("TOLA_HOOK_SCENE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
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
                "hooks::command::tests::hook_environment_child",
            ),
            HookCall {
                site_root: &root,
                name: "environment",
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
    fn hook_environment_child() {
        let Some(stage) = std::env::var_os("TOLA_HOOK_SCENE_STAGE") else {
            return;
        };
        let mode = std::env::var("TOLA_HOOK_SCENE_MODE").unwrap();

        assert_eq!(
            std::env::var_os("TOLA_BUILD_MODE").as_deref(),
            Some(std::ffi::OsStr::new(&mode))
        );
        crate::build::tests::assert_hook_environment(&stage.to_string_lossy());
    }
}
