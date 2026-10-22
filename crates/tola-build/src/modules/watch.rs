//! Read-only watch inputs for potentially enabled module hooks.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::{InputScope, config::ResolvedSiteConfig};

/// Conservative paths declared by hooks that may run in development.
/// No options are evaluated and no command is run to construct this value.
#[derive(Debug, Clone, Default)]
pub struct HookWatchInputs {
    rerun_paths: Vec<PathBuf>,
    generated_paths: Vec<PathBuf>,
}

impl HookWatchInputs {
    /// Input paths whose changes can rerun a module hook.
    pub fn rerun_paths(&self) -> &[PathBuf] {
        &self.rerun_paths
    }
    /// Before-build output paths that may need event quarantine.
    pub fn generated_paths(&self) -> &[PathBuf] {
        &self.generated_paths
    }
}

/// The module configuration one preparation froze, shared by development subscriptions and
/// a build attempt so both read the same manifests.
pub struct PreparedHookWatch {
    config: Arc<ResolvedSiteConfig>,
    inputs: HookWatchInputs,
    scope: InputScope,
}

impl PreparedHookWatch {
    /// The configuration these paths were read from.
    pub fn config(&self) -> &Arc<ResolvedSiteConfig> {
        &self.config
    }
    /// Conservative watch paths declared by that same configuration.
    pub fn inputs(&self) -> &HookWatchInputs {
        &self.inputs
    }
    pub(crate) fn scope(&self) -> InputScope {
        self.scope
    }
}

/// Freeze the module configuration and derive development watch paths without running commands.
pub fn prepare_hook_watch(
    config: &ResolvedSiteConfig,
    scope: InputScope,
) -> Result<PreparedHookWatch> {
    let config = Arc::new(super::prepared::configure(config, scope)?);
    let mut inputs = HookWatchInputs::default();
    for module in config.module_inputs.configured.values() {
        super::hooks::watch_inputs(
            config.get_root(),
            &module.module,
            &mut inputs.rerun_paths,
            &mut inputs.generated_paths,
        )?;
    }
    inputs.rerun_paths.sort_unstable();
    inputs.rerun_paths.dedup();
    inputs.generated_paths.sort_unstable();
    inputs.generated_paths.dedup();
    Ok(PreparedHookWatch {
        config,
        inputs,
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{BuildRequest, BuildSession};
    use std::fs;

    #[test]
    fn prepared_watch_paths_stay_conservative() {
        let site = tempfile::tempdir().unwrap();
        let root = site.path();
        fs::create_dir(root.join("content")).unwrap();
        fs::write(root.join("site.typ"), "#document(\"index.html\")[Site]").unwrap();
        let module = root.join("module");
        fs::create_dir(&module).unwrap();
        let manifest = module.join("tola-module.toml");
        fs::write(
            &manifest,
            r#"
schema-version = 1
[[hooks]]
stage = "before-build"
name = "conditional"
command = ["this-must-not-run"]
enabled-by = "enabled"
rerun-on = [{module = "tool.txt"}, {project = "input/{name}"}]
generates = ["generated/{name}.txt"]
[[hooks]]
stage = "after-publish"
name = "disabled-in-dev"
command = ["unused"]
rerun-on = [{project = "not-watched"}]
"#,
        )
        .unwrap();
        let config =
            crate::config::tests::load_test_config(root, "[modules.notes]\npath=\"module\"\n");
        let prepared = prepare_hook_watch(&config, InputScope::Online).unwrap();
        assert!(
            prepared
                .inputs()
                .rerun_paths()
                .iter()
                .any(|path| path.ends_with("module/tool.txt"))
        );
        assert!(
            prepared
                .inputs()
                .rerun_paths()
                .iter()
                .any(|path| path.ends_with("input/notes"))
        );
        assert!(
            prepared
                .inputs()
                .generated_paths()
                .iter()
                .any(|path| path.ends_with("generated/notes.txt"))
        );
        assert!(
            !prepared
                .inputs()
                .rerun_paths()
                .iter()
                .any(|path| path.ends_with("not-watched"))
        );
        let mut session = BuildSession::new();
        let attempt = session
            .prepare_with_hook_watch(&prepared, BuildRequest::default())
            .unwrap();
        fs::write(&manifest, "schema-version = 1\n# changed before hook\n").unwrap();
        let Err(failure) = attempt.run() else {
            panic!("a changed manifest must invalidate the prepared attempt");
        };
        assert!(format!("{failure:#}").contains("the site's modules changed"));
        assert!(!root.join("generated").exists());
    }

    #[test]
    fn prepared_cannot_cross_input_scope() {
        let site = tempfile::tempdir().unwrap();
        let config = crate::config::tests::load_test_config(site.path(), "");
        let prepared = prepare_hook_watch(&config, InputScope::Online).unwrap();
        let mut pure = BuildSession::with_resources(
            crate::BuildResources::new().with_input_scope(InputScope::Pure),
        );
        assert!(
            pure.prepare_with_hook_watch(&prepared, BuildRequest::default())
                .is_err()
        );
    }
}
