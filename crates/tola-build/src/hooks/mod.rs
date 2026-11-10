//! Trusted build scripts and their process lifecycle.

mod command;
mod evidence;
mod generate;
mod pack;
mod report;
mod reuse;
mod runner;

pub(crate) use crate::config::section::build::hooks::hook_identity;
pub use command::{HookRunner, NuHook};
pub use evidence::{FailedHookOutputs, HookOutputEvidence, SourceHookOutputs};
pub(crate) use generate::generate_outputs;
pub use pack::run_before_pack;
pub use reuse::AfterPublishDependencies;
pub(crate) use reuse::{ReusableHookOutputs, observe_after_publish};
pub use runner::*;

use crate::config::section::build::{DevParticipation, HooksConfig, hooks::HookStage};
use crate::mode::BuildMode;

/// An enabled hook command announced by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredHook<'a> {
    pub stage: HookStage,
    /// Required name from the hook declaration.
    pub label: &'a str,
    dev: DevParticipation,
}

impl ConfiguredHook<'_> {
    /// Whether this announced hook runs in `mode`; announcements hold enabled hooks only.
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
        .map(|hook| announcement(HookStage::BeforeBuild, &hook.name, hook.dev));
    let generate_outputs = hooks
        .generate_outputs
        .iter()
        .filter(|command| command.enable)
        .map(|command| announcement(HookStage::GenerateOutputs, &command.name, command.dev));
    let after_publish = hooks
        .after_publish
        .iter()
        .filter(|hook| hook.enable)
        .map(|hook| announcement(HookStage::AfterPublish, &hook.name, hook.dev));
    before_build.chain(generate_outputs).chain(after_publish)
}

/// One enabled hook of any stage, as the host announces it.
fn announcement<'a>(stage: HookStage, label: &'a str, dev: DevParticipation) -> ConfiguredHook<'a> {
    ConfiguredHook { stage, label, dev }
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
