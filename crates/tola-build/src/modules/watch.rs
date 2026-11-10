//! Build-side prepared hook watch.

use anyhow::Result;

use tola_module::watch::HookWatchInputs;

use crate::config::ResolvedSiteConfig;
use tola_input::{InputMode, InputScope};

/// The module configuration one preparation froze, shared by development subscriptions and
/// a build attempt so both read the same manifests.
pub struct PreparedHookWatch {
    site: crate::modules::prepared::PreparedSite,
    scope: InputScope,
}

impl PreparedHookWatch {
    /// The configuration this preparation applied.
    pub fn config(&self) -> &ResolvedSiteConfig {
        self.site.config()
    }
    /// The applied configuration as the build's shared handle.
    pub fn config_arc(&self) -> &std::sync::Arc<ResolvedSiteConfig> {
        self.site.config_arc()
    }
    /// The prepared contribution these paths were derived from.
    pub fn site(&self) -> &crate::modules::prepared::PreparedSite {
        &self.site
    }
    /// Conservative watch paths declared by that same configuration.
    pub fn inputs(&self) -> &HookWatchInputs {
        &self.site.contribution().watch
    }
    pub(crate) fn scope(&self) -> InputScope {
        self.scope
    }
}

/// Freeze the module configuration and derive development watch paths without running commands.
pub fn prepare_hook_watch(
    config: &ResolvedSiteConfig,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<PreparedHookWatch> {
    let site = crate::modules::prepared::prepare_site(config, mode, cancellation)?;
    Ok(PreparedHookWatch {
        site,
        scope: mode.scope(),
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
[options]
enabled = false

[[hooks.before-build]]
name = "conditional"
argv = ["this-must-not-run"]
enabled-by = "enabled"
rerun-on = { include = ["tool.txt", "data/**/*.json", {project = "input/{name}"}], exclude = [{project = "module/data/vendor"}] }
generates = ["generated/{name}.txt"]
[[hooks.after-publish]]
name = "disabled-in-dev"
argv = ["unused"]
rerun-on = { include = [{project = "not-watched"}] }
"#,
        )
        .unwrap();
        let config =
            crate::config::tests::load_test_config(root, "[modules.notes]\npath=\"module\"\n");
        let prepared = prepare_hook_watch(
            &config,
            InputMode::new(InputScope::Online, false),
            &Default::default(),
        )
        .unwrap();
        let rerun_roots: Vec<_> = prepared
            .inputs()
            .rerun_selections()
            .iter()
            .flat_map(tola_module::hooks::RerunSelection::roots)
            .collect();
        assert!(
            rerun_roots
                .iter()
                .any(|path| path.ends_with("module/tool.txt"))
        );
        assert!(rerun_roots.iter().any(|path| path.ends_with("input/notes")));
        let selection = &prepared.inputs().rerun_selections()[0];
        let module = prepared.config().get_root().join("module");
        assert!(selection.matches(&module.join("data/entry.json")));
        assert!(!selection.matches(&module.join("data/vendor/entry.json")));
        assert!(
            prepared
                .inputs()
                .generated_paths()
                .iter()
                .any(|path| path.ends_with("generated/notes.txt"))
        );
        assert!(!rerun_roots.iter().any(|path| path.ends_with("not-watched")));
        let mut session = BuildSession::new();
        let attempt = session
            .prepare_with_hook_watch(&prepared, BuildRequest::default())
            .unwrap();
        fs::write(&manifest, "# changed before hook\n").unwrap();
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
        let prepared = prepare_hook_watch(
            &config,
            InputMode::new(InputScope::Online, false),
            &Default::default(),
        )
        .unwrap();
        let mut pure = BuildSession::with_resources(
            crate::BuildResources::new().with_input_scope(InputScope::Pure),
        );
        assert!(
            pure.prepare_with_hook_watch(&prepared, BuildRequest::default())
                .is_err()
        );
    }
}
