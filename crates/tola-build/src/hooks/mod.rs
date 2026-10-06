//! Trusted build scripts and their process lifecycle.

mod command;
mod evidence;
mod generate;
mod report;
mod runner;

pub(crate) use crate::config::section::build::hooks::hook_identity;
pub use evidence::{FailedHookOutputs, HookOutputEvidence, SourceHookOutputs};
pub(crate) use generate::generate_outputs;
pub use runner::*;

use crate::config::section::build::DevParticipation;
use crate::config::section::build::HooksConfig;
use crate::config::section::build::hooks::HookStage;
use crate::mode::BuildMode;

/// An enabled hook command announced by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredHook<'a> {
    pub stage: HookStage,
    /// Required name from the hook declaration.
    pub label: &'a str,
    pub command: &'a [String],
    dev: DevParticipation,
}

impl ConfiguredHook<'_> {
    pub fn participates(&self, mode: BuildMode) -> bool {
        hook_participates(true, self.dev, mode)
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
            command: &hook.command,
            dev: hook.dev,
        });
    let generate_outputs = hooks
        .generate_outputs
        .iter()
        .filter(|command| command.enable)
        .map(|command| ConfiguredHook {
            stage: HookStage::GenerateOutputs,
            label: &command.name,
            command: &command.command,
            dev: command.dev,
        });
    let after_publish = hooks
        .after_publish
        .iter()
        .filter(|hook| hook.enable)
        .map(|hook| ConfiguredHook {
            stage: HookStage::AfterPublish,
            label: &hook.name,
            command: &hook.command,
            dev: hook.dev,
        });
    before_build.chain(generate_outputs).chain(after_publish)
}

pub(super) fn hook_participates(
    enable: bool,
    dev: DevParticipation,
    mode: crate::mode::BuildMode,
) -> bool {
    enable
        && match mode {
            crate::mode::BuildMode::Production => true,
            crate::mode::BuildMode::Development => dev.participates_in_development(),
        }
}

pub(super) fn hook_contract_error(
    message: impl Into<String>,
    help: impl Into<String>,
) -> anyhow::Error {
    let message = message.into();
    let diagnostic = crate::diagnostic::Diagnostic::new(
        crate::codes::build::HOOKS,
        crate::diagnostic::Severity::Error,
        message.clone(),
    )
    .with_help(help);
    anyhow::Error::new(crate::diagnostic::DiagnosticError::new(
        message,
        vec![diagnostic],
    ))
}

/// Whether candidate construction runs its selected pre-publication commands.
///
/// Publication consumers execute separately against a committed output view.
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
command = ["tailwindcss", "-i", "static/css/site.css"]

[[before-build]]
name = "retired"
enable = false
command = ["unused"]

[[before-build]]
name = "sitemap"
command = ["node", "scripts/sitemap.mjs"]
dev = "skip"

[[generate-outputs]]
name = "pagefind"
command = ["pagefind"]
outputs = [{ tree = "search" }]

[[after-publish]]
name = "deploy"
command = ["scripts/deploy.sh"]
"#,
        );

        let listed = configured_hooks(&hooks)
            .map(|hook| {
                (
                    hook.stage.as_str(),
                    hook.label.to_owned(),
                    hook.command.iter().map(String::as_str).collect::<Vec<_>>(),
                    hook.participates(BuildMode::Development),
                    hook.participates(BuildMode::Production),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            listed,
            [
                (
                    "before-build",
                    "tailwind".to_owned(),
                    vec!["tailwindcss", "-i", "static/css/site.css"],
                    true,
                    true
                ),
                (
                    "before-build",
                    "sitemap".to_owned(),
                    vec!["node", "scripts/sitemap.mjs"],
                    false,
                    true
                ),
                (
                    "generate-outputs",
                    "pagefind".to_owned(),
                    vec!["pagefind"],
                    true,
                    true
                ),
                (
                    "after-publish",
                    "deploy".to_owned(),
                    vec!["scripts/deploy.sh"],
                    false,
                    true
                ),
            ]
        );
    }
}
