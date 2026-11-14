//! Module hook tests; the hook plan itself lives in tola-module.

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use tola_module::hooks::*;

    fn loaded_modules(
        site: &std::path::Path,
        declarations: &std::collections::BTreeMap<String, tola_module::ModuleDeclaration>,
        mode: tola_input::InputMode,
    ) -> std::result::Result<
        std::collections::BTreeMap<String, tola_module::module_set::LoadedModule>,
        tola_module::module_set::ModuleLoadError,
    > {
        let roots =
            crate::inputs::module_roots(site, mode, None, declarations, &Default::default())?;
        crate::modules::load_modules_with_roots(site, declarations, &roots, mode)
    }

    fn parse(source: &str) -> std::result::Result<ModuleHooks, toml::de::Error> {
        #[derive(Deserialize)]
        struct Document {
            hooks: ModuleHooks,
        }
        toml::from_str::<Document>(source).map(|document| document.hooks)
    }

    #[test]
    fn module_hooks_validate_inactive_declarations() {
        for source in [
            "[[hooks.generate-outputs]]\nname = 'x'\nargv = ['x']\noutputs = []\n",
            "[[hooks.before-build]]\nname = 'x'\nargv = ['x']\ndev = 'reuse'\n",
            "[[hooks.before-build]]\nname = 'x'\nargv = ['x']\nrerun-on = { include = ['../escape'] }\n",
        ] {
            assert!(
                validate_declarations(&parse(source).unwrap()).is_err(),
                "{source}"
            );
        }
        for source in [
            "[[hooks.before-build]]\nname = 'x'\nargv = ['x']\nenabled-by = 'flag'\noutputs = []\n",
            "[[hooks.after-publish]]\nname = 'x'\nargv = ['x']\ngenerates = []\n",
            "[[hooks.mystery]]\nname = 'x'\nargv = ['x']\n",
            "[[hooks.before-build]]\nname = 'x'\nargv = ['x']\nextra = 1\n",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }

    mod module_hooks {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        use std::path::{Path, PathBuf};
        use std::sync::Arc;

        use crate::build::BuildRequest;
        use crate::cancellation::BuildCancellation;
        use crate::config::ResolvedSiteConfig;
        use crate::config::loading::{BuildOverrides, load_site_config};
        use crate::{BuildResources, BuildSession, modules};
        use tola_input::InputScope;

        fn write(root: &Path, path: &str, text: &str) {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn command() -> String {
            let exe = std::env::current_exe().unwrap();
            format!(
                "[{}, \"--exact\", \"modules::hooks::tests::module_hooks::module_hook_child\", \"--nocapture\"]",
                serde_json::to_string(&exe.to_string_lossy()).unwrap()
            )
        }

        fn describe(error: &anyhow::Error) -> String {
            format!(
                "{error:#}\nattached diagnostics: {:#?}",
                tola_diagnostic::attached(error)
            )
        }

        struct Site {
            _workspace: tempfile::TempDir,
            root: PathBuf,
        }
        impl Site {
            fn new(generates: &str, action: &str, extra: &str) -> Self {
                let workspace = tempfile::tempdir().unwrap();
                let root = workspace.path().join("project");
                let module = workspace.path().join("module");
                fs::create_dir_all(&root).unwrap();
                fs::create_dir_all(&module).unwrap();
                fs::create_dir_all(root.join("content")).unwrap();
                write(
                    &module,
                    "tola-module.toml",
                    &format!(
                        r#"
        [[hooks.before-build]]
        name = "prepare"
        argv = {}
        enabled-by = "enabled"
        generates = [{generates}]
        [queries.nodes]
        entry = "query.typ"
        label = "nodes"
                "#,
                        command()
                    ),
                );
                write(&module, "dependency.txt", "original");
                write(&module, "query.typ", "#metadata((value: 1))<nodes>\n");
                write(
                    &root,
                    "site.typ",
                    "#document(\"index.html\", format: \"html\")[OK]\n",
                );
                let options = if extra.contains("[modules.demo.options]") {
                    String::new()
                } else {
                    format!(
                        "[modules.demo.options]\nenabled = true\naction = \"{action}\"\ndependency = \"original\"\n"
                    )
                };
                write(
                    &root,
                    "tola.toml",
                    &format!(
                        r#"[modules.demo]
        path = "../module"
        [typst.fonts]
        system = false
        {options}
        {extra}
        "#
                    ),
                );
                Self {
                    _workspace: workspace,
                    root,
                }
            }
            fn config(&self) -> anyhow::Result<ResolvedSiteConfig> {
                Ok(load_site_config(
                    Some(&self.root.join("tola.toml")),
                    tola_typst::PackageLocations::default(),
                    &BuildOverrides::default(),
                )?
                .into_config())
            }
            fn session(&self) -> BuildSession {
                BuildSession::with_resources(
                    BuildResources::new().with_input_scope(InputScope::Offline),
                )
            }
            fn log(&self) -> String {
                fs::read_to_string(self.root.join("hook-log")).unwrap_or_default()
            }
        }

        fn disabled_site() -> Site {
            Site::new(
                "'generated/value.txt'",
                "normal",
                "[modules.demo.options]\nenabled = false\n",
            )
        }

        /// Executed as an independent hook child, never directly by the test harness.
        #[test]
        fn module_hook_child() {
            let Some(context) = crate::build::tests::hook_context() else {
                return;
            };
            assert_eq!(context.hook.unwrap().stage, "before-build");
            let site = context.site.unwrap();
            let root = site.root;
            assert_eq!(root, std::env::current_dir().unwrap());
            assert_eq!(
                site.config_path.canonicalize().unwrap(),
                root.join("tola.toml").canonicalize().unwrap()
            );
            assert_eq!(site.content_root, root.join("content"));
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("hook-log"))
                .unwrap();
            match context.module {
                None => {
                    writeln!(log, "project").unwrap();
                }
                Some(module) => {
                    assert_eq!(module.name, "demo");
                    let module_root = module.root;
                    assert_eq!(
                        module_root,
                        root.parent()
                            .unwrap()
                            .join("module")
                            .canonicalize()
                            .unwrap()
                    );
                    assert_eq!(module.manifest, module_root.join("tola-module.toml"));
                    let options = module.options.unwrap();
                    assert_eq!(options["enabled"], true);
                    assert_eq!(options["dependency"], "original");
                    writeln!(log, "module:{}", options["action"].as_str().unwrap()).unwrap();
                    fs::create_dir_all(root.join("generated/fonts")).unwrap();
                    fs::write(root.join("generated/value.txt"), "generated").unwrap();
                    if options["action"] == "stale" {
                        fs::write(module_root.join("dependency.txt"), "changed").unwrap();
                    }
                }
            }
        }

        #[test]
        fn defaults_and_repeat_build() {
            let project_hook = format!(
                "[[hooks.before-build]]\nname = 'project'\nargv = {}\n",
                command()
            );
            let site = Site::new("'generated/value.txt'", "normal", &project_hook);
            let config = modules::prepared::prepare_site(
                &site.config().unwrap(),
                tola_input::InputMode::new(InputScope::Offline, false),
                &Default::default(),
            )
            .unwrap();
            let mut session = site.session();
            session
                .build_and_write(&config, BuildRequest::default())
                .unwrap_or_else(|error| panic!("first build: {}", describe(&error)));
            session
                .build_and_write(&config, BuildRequest::default())
                .unwrap_or_else(|error| panic!("second build: {}", describe(&error)));
            assert_eq!(
                site.log(),
                "project\nmodule:normal\nproject\nmodule:normal\n"
            );
            assert!(site.root.join("public/index.html").is_file());
        }

        #[test]
        fn query_reports_unverified_generation() {
            let site = disabled_site();
            let config = Arc::new(site.config().unwrap());
            let resolved = super::loaded_modules(
                config.get_root(),
                config.modules(),
                tola_input::InputMode::new(InputScope::Offline, false),
            )
            .unwrap();
            let module = &resolved["demo"];
            let result = modules::query::run(
                Arc::clone(&config),
                module,
                &module.queries["nodes"],
                "demo.nodes",
                BuildResources::new().with_input_scope(InputScope::Offline),
                vec![(site.root.join("site.typ"), Arc::from("Overlay content"))],
                &BuildCancellation::default(),
            )
            .unwrap();
            assert!(result.success, "{:?}", result.diagnostics);
            assert_eq!(result.input.hooks, "skipped");
            assert_eq!(result.input.mode, "overlay");
            assert_eq!(result.input.generation_status, "unverified");
            assert!(
                result
                    .input
                    .generated
                    .iter()
                    .any(|item| item.producer == "@module/demo/prepare"
                        && item.path == "generated/value.txt")
            );
            assert_eq!(site.log(), "");
        }
    }
}
