//! Trusted build scripts and their process lifecycle.

pub(crate) mod active;
mod command;
mod evidence;
mod executor;
mod generate;
mod pack;
mod report;
mod reuse;
mod runner;
mod schedule;

pub(crate) use crate::config::section::build::hooks::hook_identity;
pub use command::{HookRunner, NuHook, expand_hook_argument};
pub use evidence::{FailedHookOutputs, HookOutputEvidence, SourceHookOutputs};
pub use executor::HookExecutor;
pub(crate) use generate::generate_outputs;
pub use pack::{before_pack_context, run_before_pack};
pub use reuse::AfterPublishDependencies;
pub(crate) use reuse::{PreparedHooks, ReusableHookOutputs, observe_after_publish};
pub use runner::*;
pub use schedule::{hook_batches, hook_limit};

use crate::config::section::build::{
    DevParticipation, HooksConfig,
    hooks::{HookStage, ModuleHookOrigin},
};
use crate::mode::BuildMode;
use std::path::Path;

/// The hook whose invocation catches failures on the calling thread.
/// Hosts use this to keep panic details in their log without printing a second diagnostic.
pub fn current_hook_stage() -> Option<HookStage> {
    active::stage()
}

/// An enabled hook command announced by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredHook<'a> {
    pub stage: HookStage,
    /// Required name from the hook declaration.
    pub label: &'a str,
    dev: DevParticipation,
    argv: Option<&'a [String]>,
    /// The module that declares the hook, if any.
    pub origin: Option<&'a ModuleHookOrigin>,
}

impl ConfiguredHook<'_> {
    /// Whether this announced hook runs in `mode`; announcements hold enabled hooks only.
    pub fn participates(&self, mode: BuildMode) -> bool {
        hook_participates(true, self.dev, mode)
    }

    /// The executable as written in `argv`, before expanding placeholders.
    pub fn program(&self) -> Option<&str> {
        self.argv?.first().map(String::as_str)
    }
}

/// Announcements omit disabled hooks and follow lifecycle order.
pub fn configured_hooks(hooks: &HooksConfig) -> impl Iterator<Item = ConfiguredHook<'_>> {
    let before_build = hooks
        .before_build
        .iter()
        .filter(|hook| hook.enable)
        .map(|hook| ConfiguredHook {
            stage: HookStage::BeforeBuild,
            label: &hook.name,
            dev: hook.dev,
            argv: hook.argv.as_deref(),
            origin: hook.origin.as_ref(),
        });
    let generate_outputs = hooks
        .generate_outputs
        .iter()
        .filter(|command| command.enable)
        .map(|hook| ConfiguredHook {
            stage: HookStage::GenerateOutputs,
            label: &hook.name,
            dev: hook.dev,
            argv: hook.argv.as_deref(),
            origin: hook.origin.as_ref(),
        });
    let after_publish = hooks
        .after_publish
        .iter()
        .filter(|hook| hook.enable)
        .map(|hook| ConfiguredHook {
            stage: HookStage::AfterPublish,
            label: &hook.name,
            dev: hook.dev,
            argv: hook.argv.as_deref(),
            origin: hook.origin.as_ref(),
        });
    before_build.chain(generate_outputs).chain(after_publish)
}

/// Site-relative inputs declared by enabled before-build hooks, after module hooks are installed.
pub fn generated_inputs(hooks: &HooksConfig) -> impl Iterator<Item = (&str, &Path)> {
    hooks
        .before_build
        .iter()
        .filter(|hook| hook.enable)
        .flat_map(|hook| {
            hook.generates
                .iter()
                .map(|path| (hook.name.as_str(), path.as_path()))
        })
}

/// The enabled hook declaring this missing site input, directly or through its directory.
pub fn generated_input_producer<'a>(
    hooks: &'a HooksConfig,
    root: &Path,
    path: &Path,
) -> Option<&'a str> {
    let relative = path.strip_prefix(root).ok()?;
    generated_inputs(hooks).find_map(|(name, generated)| {
        let generated = generated.strip_prefix(".").unwrap_or(generated);
        relative.starts_with(generated).then_some(name)
    })
}

/// Whether one declared hook contributes to this mode.
pub(super) fn hook_participates(enable: bool, dev: DevParticipation, mode: BuildMode) -> bool {
    enable
        && match mode {
            BuildMode::Production => true,
            BuildMode::Development => dev.participates_in_development(),
        }
}

pub(super) fn hook_contract_error(
    message: impl Into<String>,
    help: impl Into<String>,
) -> anyhow::Error {
    let message = message.into();
    let diagnostic = tola_diagnostic::Diagnostic::new(
        tola_diagnostic::codes::build::HOOKS,
        tola_diagnostic::Severity::Error,
        message.clone(),
    )
    .with_help(help);
    anyhow::Error::new(tola_diagnostic::DiagnosticError::new(
        message,
        vec![diagnostic],
    ))
}

/// Whether candidate construction runs its selected pre-publication commands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HookExecution {
    #[default]
    Run,
    /// Run no hook command: the attempt builds from existing inputs only.
    Skip,
}

impl HookExecution {
    #[inline]
    pub fn runs(&self) -> bool {
        matches!(self, Self::Run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> HooksConfig {
        toml::from_str(source).unwrap()
    }

    #[test]
    fn configured_hooks_follow_lifecycle_order() {
        let hooks = parse(
            r#"
[[before-build]]
name = "tailwind"
argv = ["tailwindcss", "-i", "static/css/site.css"]

[[before-build]]
name = "retired"
enable = false
argv = ["unused"]

[[before-build]]
name = "sitemap"
argv = ["node", "scripts/sitemap.mjs"]
dev = "skip"

[[generate-outputs]]
name = "pagefind"
argv = ["pagefind"]
outputs = [{ tree = "search" }]

[[after-publish]]
name = "deploy"
argv = ["scripts/deploy.sh"]
"#,
        );

        let listed = configured_hooks(&hooks)
            .map(|hook| {
                (
                    hook.stage.as_str(),
                    hook.label.to_owned(),
                    hook.participates(BuildMode::Development),
                    hook.participates(BuildMode::Production),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            listed,
            [
                ("before-build", "tailwind".to_owned(), true, true),
                ("before-build", "sitemap".to_owned(), false, true),
                ("generate-outputs", "pagefind".to_owned(), true, true),
                ("after-publish", "deploy".to_owned(), false, true),
            ]
        );
    }
}
