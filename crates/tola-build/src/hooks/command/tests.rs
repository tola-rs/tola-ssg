use super::*;
use std::process::Command as ProcessCommand;

#[test]
fn cancelled_command_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let canceller = crate::cancellation::BuildCanceller::new();
    canceller.cancel();
    let invocation = HookInvocation::new(
        &["tola-hook-missing-program"],
        HookCall {
            site: HookSitePaths {
                root: dir.path(),
                config: dir.path(),
                content: dir.path(),
                scope: tola_input::InputScope::Online,
            },
            name: "search",
            origin: None,
            mode: BuildMode::Production,
            directories: HookDirectories::BeforeBuild,
        },
    )
    .unwrap();
    let err = invocation
        .run(Some(&canceller.token()), &|| {})
        .unwrap_err();

    assert!(err.is::<crate::cancellation::BuildCancelled>());
}

#[test]
fn hook_env_overrides_inherited() {
    let dir = tempfile::tempdir().unwrap();
    let wrong = dir.path().join("wrong");
    for (stage, mode) in [
        ("before-build", "dev"),
        ("before-build", "prod"),
        ("generate-outputs", "dev"),
        ("generate-outputs", "prod"),
        ("after-publish", "dev"),
        ("after-publish", "prod"),
    ] {
        for module in [false, true] {
            let mut cmd = ProcessCommand::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "hooks::command::tests::hook_environment_scene",
                "--nocapture",
            ])
            .env("TOLA_HOOK_SCENE_ROOT", dir.path())
            .env("TOLA_HOOK_SCENE_STAGE", stage)
            .env("TOLA_HOOK_SCENE_MODE", mode)
            .env("TOLA_HOOK_SCENE_MODULE", if module { "yes" } else { "no" })
            .env("TOLA_EXECUTABLE", &wrong)
            .env(crate::context::PATH_ENV, &wrong);
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{stage} in {mode}:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[test]
fn hook_environment_scene() {
    let Some(root) = std::env::var_os("TOLA_HOOK_SCENE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let cfg = root.join("custom/config.toml");
    let content = root.join("custom/content");
    let origin = ModuleHookOrigin {
        name: "notes".parse().unwrap(),
        root: root.join("module"),
        manifest: root.join("module/tola-module.toml"),
        owner: tola_module::ModuleOwner::Site,
        options_json: br#"{"search":true}"#.to_vec(),
    };
    let is_module = std::env::var("TOLA_HOOK_SCENE_MODULE").unwrap() == "yes";
    let stage = std::env::var("TOLA_HOOK_SCENE_STAGE").unwrap();
    let mode = std::env::var("TOLA_HOOK_SCENE_MODE").unwrap();
    let input = root.join("input");
    let output = root.join("output");
    for dir in [&input, &output] {
        fs::create_dir_all(dir).unwrap();
    }
    let directories = match stage.as_str() {
        "before-build" => HookDirectories::BeforeBuild,
        "generate-outputs" => HookDirectories::GenerateOutputs {
            input: &input,
            output: &output,
            outputs: &[],
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
        &crate::build::tests::hook_child_command("hooks::command::tests::child_hook_environment"),
        HookCall {
            site: HookSitePaths {
                root: &root,
                config: &cfg,
                content: &content,
                scope: tola_input::InputScope::Pure,
            },
            name: "environment",
            origin: is_module.then_some(&origin),
            mode,
            directories,
        },
    )
    .unwrap();

    invocation.run(None, &|| {}).unwrap();
}

#[test]
fn child_hook_environment() {
    let Some(stage) = std::env::var_os("TOLA_HOOK_SCENE_STAGE") else {
        return;
    };
    let mode = std::env::var("TOLA_HOOK_SCENE_MODE").unwrap();
    let root = PathBuf::from(std::env::var_os("TOLA_HOOK_SCENE_ROOT").unwrap());
    let ctx = crate::build::tests::hook_context().unwrap();
    assert_eq!(ctx.input_scope.as_deref(), Some("pure"));
    assert_eq!(ctx.executable, std::env::current_exe().unwrap());
    assert_eq!(
        std::env::var_os("TOLA_EXECUTABLE").as_deref(),
        Some(ctx.executable.as_os_str())
    );
    let site = ctx.site.unwrap();
    assert_eq!(site.root, root);
    assert_eq!(site.config_path, root.join("custom/config.toml"));
    assert_eq!(site.content_root, root.join("custom/content"));
    let module = std::env::var("TOLA_HOOK_SCENE_MODULE").unwrap() == "yes";
    assert_eq!(ctx.module.is_some(), module);
    if let Some(module) = ctx.module {
        assert_eq!(module.name, "notes");
        assert_eq!(module.root, root.join("module"));
        assert_eq!(module.manifest, root.join("module/tola-module.toml"));
        assert_eq!(module.options.unwrap(), serde_json::json!({"search": true}));
    }
    assert_eq!(ctx.hook.unwrap().mode.as_deref(), Some(mode.as_str()));
    crate::build::tests::assert_hook_environment(&stage.to_string_lossy());
}
