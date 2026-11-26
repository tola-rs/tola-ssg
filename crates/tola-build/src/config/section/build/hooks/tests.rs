use super::*;
use crate::config::tests::diagnostic_mentioning;

fn input_generator(name: &str, out: &str) -> BeforeBuildHookConfig {
    BeforeBuildHookConfig {
        name: name.into(),
        argv: Some(vec!["generator".into()]),
        generates: vec![out.into()],
        ..BeforeBuildHookConfig::default()
    }
}

fn one_hook_per_stage(name: &str, cmd: &str) -> HooksConfig {
    HooksConfig {
        before_build: vec![BeforeBuildHookConfig {
            name: name.into(),
            argv: Some(vec![cmd.into()]),
            generates: vec!["generated".into()],
            ..BeforeBuildHookConfig::default()
        }],
        generate_outputs: vec![OutputCommandConfig {
            name: name.into(),
            argv: Some(vec![cmd.into()]),
            outputs: vec![CommandOutput::File(
                tola_address::OutputPath::parse("search.json").unwrap(),
            )],
            ..OutputCommandConfig::default()
        }],
        after_publish: vec![AfterPublishHookConfig {
            name: name.into(),
            argv: Some(vec![cmd.into()]),
            ..AfterPublishHookConfig::default()
        }],
    }
}

fn assert_hook_output_refused(root: &Path, declared: &str) {
    let mut schema = crate::config::SiteConfigSchema::default();
    schema.vendor.path = Some("vendor".into());
    schema.hooks.before_build = vec![input_generator("generator", declared)];

    let err = schema
        .resolve(
            &root.join("tola.toml"),
            tola_typst::PackageLocations::default(),
            &crate::config::loading::BuildOverrides::default(),
        )
        .unwrap_err();

    let diag = diagnostic_mentioning(&err, declared);
    assert_eq!(diag.code, tola_diagnostic::codes::config::INVALID);
    assert_eq!(
        diag.notes,
        [BeforeBuildHookConfig::FIELDS.generates.as_str()]
    );
}

#[test]
fn hook_order_nonnegative() {
    for stage in ["before-build", "generate-outputs", "after-publish"] {
        let src = format!("[[{stage}]]\nname = 'one'\norder = 7\n");
        let hooks: HooksConfig = toml::from_str(&src).unwrap();
        let order = match stage {
            "before-build" => hooks.before_build[0].order,
            "generate-outputs" => hooks.generate_outputs[0].order,
            _ => hooks.after_publish[0].order,
        };
        assert_eq!(order, 7);
        for val in ["-1", "1.5", "true", "'1'"] {
            let src = format!("[[{stage}]]\nname = 'one'\norder = {val}\n");
            assert!(toml::from_str::<HooksConfig>(&src).is_err());
        }
    }
}

#[test]
fn reuse_requires_outputs() {
    let cfg: HooksConfig = toml::from_str(
        r#"
[[before-build]]
name = "prepare"
nu = "prepare.nu"
dev = "reuse"
generates = ["generated/data.json"]

[[generate-outputs]]
name = "search"
argv = ["search"]
dev = "reuse"
outputs = [{ file = "search.json" }]
"#,
    )
    .unwrap();
    assert_eq!(cfg.before_build[0].dev, DevParticipation::Reuse);
    assert_eq!(cfg.generate_outputs[0].dev, DevParticipation::Reuse);
    let mut diag = ConfigDiagnostics::new();
    cfg.validate(&mut diag);
    assert!(diag.into_result().is_ok());

    for (src, field) in [
        (
            "[[before-build]]\nname = \"prepare\"\nargv = [\"prepare\"]\ndev = \"reuse\"\n",
            BeforeBuildHookConfig::FIELDS.generates,
        ),
        (
            "[[after-publish]]\nname = \"deploy\"\nargv = [\"deploy\"]\ndev = \"reuse\"\n",
            AfterPublishHookConfig::FIELDS.dev,
        ),
    ] {
        let cfg: HooksConfig = toml::from_str(src).unwrap();
        let mut diag = ConfigDiagnostics::new();
        cfg.validate(&mut diag);
        let err = diag.into_result().unwrap_err();
        assert_eq!(err.errors()[0].field, field);
    }
}

#[test]
fn dev_rerun_selections_follow_participation() {
    let cfg: HooksConfig = toml::from_str(
        r#"
[[before-build]]
name = "prepare"
argv = ["prepare"]
rerun-on = { include = [" before "] }

[[before-build]]
name = "production-prepare"
argv = ["production-prepare"]
dev = "skip"
rerun-on = { include = ["production-input"] }

[[before-build]]
name = "disabled-prepare"
argv = ["disabled-prepare"]
enable = false
rerun-on = { include = ["disabled-input"] }

[[generate-outputs]]
name = "generate"
argv = ["generate"]
rerun-on = { include = ["generated/*.json"] }
outputs = [{ file = "search.json" }]

[[generate-outputs]]
name = "disabled-generate"
argv = ["disabled-generate"]
enable = false
rerun-on = { include = ["disabled-generated"] }

[[after-publish]]
name = "production-consumer"
argv = ["production-consumer"]
rerun-on = { include = ["production-consumer-input"] }

[[after-publish]]
name = "development-consumer"
argv = ["development-consumer"]
dev = "run"
rerun-on = { include = ["consumer"] }

[[after-publish]]
name = "disabled-consumer"
argv = ["disabled-consumer"]
enable = false
dev = "run"
rerun-on = { include = ["disabled-consumer-input"] }
"#,
    )
    .unwrap();
    let root = std::path::absolute("test-site").unwrap();
    let selections = cfg.development_rerun_selections(&root).unwrap();
    assert_eq!(
        selections
            .iter()
            .flat_map(RerunSelection::roots)
            .collect::<Vec<_>>(),
        [" before ", "generated", "consumer"].map(|p| root.join(p))
    );
    assert!(selections[1].matches(&root.join("generated/search.json")));
    assert!(!selections[1].matches(&root.join("generated/search.txt")));
}

#[test]
fn disabled_hooks_validated() {
    let cfg: HooksConfig = toml::from_str(
        r#"
[[before-build]]
enable = false
argv = [" "]
generates = ["../source"]

[[generate-outputs]]
enable = false
name = " "
rerun-on = { include = ["/outside"] }

[[after-publish]]
enable = false
rerun-on = { include = ["../committed"] }
"#,
    )
    .unwrap();
    let mut diag = ConfigDiagnostics::new();
    cfg.validate(&mut diag);
    crate::config::loading::validate_hook_output_boundaries(
        &cfg,
        Path::new("/site"),
        [Path::new("/site/tola.toml")],
        Path::new("/site/public"),
        None,
        None,
        &mut diag,
    );
    for field in [
        BeforeBuildHookConfig::FIELDS.argv,
        BeforeBuildHookConfig::FIELDS.generates,
        OutputCommandConfig::FIELDS.name,
        OutputCommandConfig::FIELDS.rerun_on,
        AfterPublishHookConfig::FIELDS.rerun_on,
    ] {
        assert!(diag.errors().iter().any(|err| err.field == field));
    }
}

#[test]
fn stage_output_shape_checked() {
    for src in [
        "[[before-build]]\ngenerates = [{ file = \"index.json\" }]\n",
        "[[generate-outputs]]\noutputs = [\"index.json\"]\n",
        "[[after-publish]]\noutputs = [\"index.json\"]\n",
    ] {
        assert!(toml::from_str::<HooksConfig>(src).is_err(), "{src}");
    }
}

#[test]
fn rerun_paths_site_relative() {
    let cfg: HooksConfig = toml::from_str(
        r#"
[[before-build]]
name = "echo-bad"
argv = ["echo", "bad"]
rerun-on = { include = ["/src"], exclude = [{ project = "../templates" }] }
"#,
    )
    .unwrap();
    let mut diag = ConfigDiagnostics::new();
    cfg.validate(&mut diag);
    assert_eq!(diag.errors().len(), 2);
    assert!(
        diag.errors()
            .iter()
            .all(|err| err.field == BeforeBuildHookConfig::FIELDS.rerun_on),
        "{:?}",
        diag.errors()
    );
}

#[test]
fn hook_name_single_word() {
    for name in ["", " ", "build stylesheet", "build\tstylesheet"] {
        let hooks = one_hook_per_stage(name, "generator");
        let mut diag = ConfigDiagnostics::new();
        hooks.validate(&mut diag);
        assert_eq!(
            diag.errors()
                .iter()
                .map(|err| err.field)
                .collect::<Vec<_>>(),
            [
                BeforeBuildHookConfig::FIELDS.name,
                OutputCommandConfig::FIELDS.name,
                AfterPublishHookConfig::FIELDS.name,
            ],
            "{name:?}",
        );
    }
}

#[test]
fn hook_name_unique_per_stage() {
    let mut hooks = one_hook_per_stage("styles", "generator");
    let mut diag = ConfigDiagnostics::new();
    hooks.validate(&mut diag);
    assert!(diag.errors().is_empty());

    hooks.before_build.push(hooks.before_build[0].clone());
    hooks
        .generate_outputs
        .push(hooks.generate_outputs[0].clone());
    hooks.after_publish.push(hooks.after_publish[0].clone());
    let mut diag = ConfigDiagnostics::new();
    hooks.validate(&mut diag);
    assert_eq!(
        diag.errors()
            .iter()
            .map(|err| err.field)
            .collect::<Vec<_>>(),
        [
            BeforeBuildHookConfig::FIELDS.name,
            OutputCommandConfig::FIELDS.name,
            AfterPublishHookConfig::FIELDS.name,
        ],
    );
}

#[test]
fn before_build_generates_refuses_owned_roots() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for declared in [
        "public/assets",
        ".tola/cache",
        ".vendor-swap",
        ".vendor-swap/marker.txt",
    ] {
        assert_hook_output_refused(root, declared);
    }

    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let workspace = root.join(".vendor-swap");
        std::fs::create_dir(&workspace).unwrap();
        std::os::unix::fs::symlink(&workspace, root.join("vendored")).unwrap();
        assert_hook_output_refused(root, "vendored/marker.txt");
    }
}

#[test]
fn before_build_generates_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let hooks = HooksConfig {
        before_build: vec![
            input_generator("directory", "generated"),
            input_generator("file", "generated/site.css"),
        ],
        ..HooksConfig::default()
    };
    let mut diag = ConfigDiagnostics::new();

    crate::config::loading::validate_hook_output_boundaries(
        &hooks,
        root,
        [root.join("tola.toml").as_path()],
        &root.join("public"),
        None,
        None,
        &mut diag,
    );

    assert_eq!(diag.errors().len(), 1);
    assert_eq!(
        diag.errors()[0].field,
        BeforeBuildHookConfig::FIELDS.generates
    );
}
