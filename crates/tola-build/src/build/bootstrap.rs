//! Install hooks selected by the site's and modules' effective options.

use std::collections::BTreeSet;

use anyhow::{Result, ensure};

use crate::config::ConfigDiagnostics;
use crate::config::section::build::hooks::{
    AfterPublishHookConfig, BeforeBuildHookConfig, HookStage, OutputCommandConfig,
};
use crate::modules::prepared::PreparedSite;

pub(crate) fn install(site: &mut PreparedSite, resources: &crate::BuildResources) -> Result<()> {
    let modules = site.prepared().clone();
    let module_hooks = tola_module::hooks::hook_stages(site.get_root(), &modules.modules)?;
    let config = site.config_mut();
    // A rebuild can start from the last published configuration, which includes module hooks.
    let mut project_hooks = config.hooks.clone();
    project_hooks
        .before_build
        .retain(|hook| hook.origin.is_none());
    project_hooks
        .generate_outputs
        .retain(|hook| hook.origin.is_none());
    project_hooks
        .after_publish
        .retain(|hook| hook.origin.is_none());
    let mut hooks = crate::config::section::build::hooks::HooksConfig {
        before_build: project_hooks.before_build,
        ..Default::default()
    };
    for entry in module_hooks.entries() {
        let origin = Some(entry.origin.clone());
        match entry.stage {
            HookStage::BeforeBuild => {
                let mut hook = BeforeBuildHookConfig {
                    origin,
                    name: entry.name.clone(),
                    dev: entry.dev,
                    rerun_on: entry.rerun_on.clone(),
                    generates: entry.generates.clone(),
                    ..Default::default()
                };
                hook.set_action(entry.action.clone());
                hooks.before_build.push(hook);
            }
            HookStage::GenerateOutputs => {
                let mut hook = OutputCommandConfig {
                    origin,
                    name: entry.name.clone(),
                    dev: entry.dev,
                    rerun_on: entry.rerun_on.clone(),
                    outputs: entry.outputs.clone(),
                    ..Default::default()
                };
                hook.set_action(entry.action.clone());
                hooks.generate_outputs.push(hook);
            }
            HookStage::AfterPublish => {
                let mut hook = AfterPublishHookConfig {
                    origin,
                    name: entry.name.clone(),
                    dev: entry.dev,
                    rerun_on: entry.rerun_on.clone(),
                    ..Default::default()
                };
                hook.set_action(entry.action.clone());
                hooks.after_publish.push(hook);
            }
        }
    }
    hooks
        .generate_outputs
        .extend(project_hooks.generate_outputs);
    hooks.after_publish.extend(project_hooks.after_publish);
    let mut named = BTreeSet::new();
    for hook in crate::hooks::configured_hooks(&hooks) {
        ensure!(
            named.insert((hook.stage.as_str(), hook.label)),
            "`{}` declares two hooks named `{}`",
            hook.stage.as_str(),
            hook.label,
        );
    }
    let mut diagnostics = ConfigDiagnostics::new();
    crate::config::loading::validate_hook_output_boundaries(
        &hooks,
        config.get_root(),
        config.config_path(),
        &config.build.publish_dir,
        config.vendor.path.as_deref(),
        config.vendor_workspace.as_deref(),
        &mut diagnostics,
    );
    diagnostics.into_result()?;
    config.hooks = hooks;
    config.hook_runner = config.hook_runner.take().map(|runner| {
        runner
            .with_modules(modules)
            .with_resources(resources.clone())
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rebuilding_replaces_module_hooks() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let module = root.join("theme");
        fs::create_dir(&module).unwrap();
        let manifest = module.join("tola-module.toml");
        fs::write(
            &manifest,
            r#"
[[hooks.before-build]]
name = "prepare"
argv = ["prepare"]

[[hooks.generate-outputs]]
name = "search"
argv = ["index"]
outputs = [{ file = "search.json" }]

[[hooks.after-publish]]
name = "notify"
argv = ["notify"]
"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(
            root,
            r#"
[modules.theme]
path = "theme"

[[hooks.before-build]]
name = "prepare"
argv = ["prepare-project"]
"#,
        );
        let resources = crate::BuildResources::new();
        let prepare = |config| {
            crate::modules::prepared::prepare_site(
                config,
                resources.input_mode(),
                &Default::default(),
            )
            .unwrap()
        };
        let mut first = prepare(&config);
        install(&mut first, &resources).unwrap();

        fs::write(
            &manifest,
            r#"
[[hooks.before-build]]
name = "prepare"
argv = ["prepare-updated"]
"#,
        )
        .unwrap();
        let mut rebuilt = prepare(first.config());
        install(&mut rebuilt, &resources).unwrap();

        let hooks = rebuilt.config().hooks();
        let names: Vec<_> = crate::hooks::configured_hooks(hooks)
            .map(|hook| hook.label)
            .collect();
        assert_eq!(names, ["prepare", "@module/theme/prepare"]);
        assert_eq!(
            hooks.before_build[1].argv.as_deref().unwrap(),
            ["prepare-updated"]
        );
    }
}
