use super::*;
use crate::build::tests::{hook_child_command, is_hook_child_for};
use crate::hooks::HookExecution;
use std::fs;
use tempfile::TempDir;

fn cfg_with_publish_dir(dir: &TempDir) -> SiteConfig {
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    fs::create_dir_all(&cfg.build.publish_dir).unwrap();
    cfg
}

fn hook(test: &str) -> BeforeBuildHookConfig {
    hook_named(test, &format!("hooks::runner::tests::{test}"))
}

fn hook_named(name: &str, child: &str) -> BeforeBuildHookConfig {
    BeforeBuildHookConfig {
        name: name.into(),
        argv: Some(hook_child_command(child)),
        ..BeforeBuildHookConfig::default()
    }
}

#[test]
fn child_writes_declared_output() {
    if !is_hook_child_for("before-build") {
        return;
    }
    crate::build::tests::assert_hook_environment("before-build");
    fs::create_dir_all("generated").unwrap();
    fs::write("generated/site.css", "compiled").unwrap();
}

#[test]
fn child_counts_successful_runs() {
    if !is_hook_child_for("before-build") {
        return;
    }
    fs::create_dir_all("generated").unwrap();
    let count = fs::read_to_string("generated/count.txt")
        .ok()
        .map(|text| text.parse::<usize>().unwrap())
        .unwrap_or_default();
    fs::write("generated/count.txt", (count + 1).to_string()).unwrap();
}

#[test]
fn prepare_reuse_inputs_and_disk() {
    use crate::config::section::build::DevParticipation;
    let dir = TempDir::new().unwrap();
    let mut cfg = cfg_with_publish_dir(&dir);
    let mut prepare = hook("child_counts_successful_runs");
    prepare.dev = DevParticipation::Reuse;
    prepare.generates = vec!["generated/count.txt".into()];
    prepare
        .rerun_on
        .include
        .push(tola_module::hooks::RerunPath::Relative(
            "inputs/*.txt".into(),
        ));
    cfg.hooks.before_build.push(prepare);
    let mut cache = ReadyHooks::default();
    let executor = HookExecutor::new();
    let mut run = |mode| {
        run_before_build_hooks_with_cancellation(
            &cfg,
            mode,
            tola_input::InputScope::Online,
            &mut cache,
            None,
            &mut Vec::new(),
            &executor,
        )
        .unwrap()
        .hooks_executed()
    };
    let dev = BuildMode::Development;
    let root = cfg.root();
    let generated = root.join("generated/count.txt");
    assert!(run(dev));
    assert!(!run(dev));
    fs::create_dir(root.join("inputs")).unwrap();
    let input = root.join("inputs/new.txt");
    fs::write(&input, "new").unwrap();
    assert!(run(dev));
    fs::write(&input, "edited").unwrap();
    assert!(run(dev));
    fs::remove_file(input).unwrap();
    assert!(run(dev));
    fs::write(&generated, "20").unwrap();
    assert!(run(dev));
    assert_eq!(fs::read_to_string(&generated).unwrap(), "21");
    fs::remove_file(&generated).unwrap();
    assert!(run(dev));
    assert_eq!(fs::read_to_string(&generated).unwrap(), "1");
    assert!(!run(dev));
    assert!(run(BuildMode::Production));
}

#[test]
fn child_writes_middle_chain_output() {
    if !is_hook_child_for("before-build") {
        return;
    }
    let count = fs::read_to_string("generated/count.txt").unwrap();
    fs::write("generated/middle.txt", count).unwrap();
    assert!(!Path::new("fail.txt").exists(), "middle command failed");
}

#[test]
fn child_writes_final_chain_output() {
    if !is_hook_child_for("before-build") {
        return;
    }
    fs::write(
        "generated/final.txt",
        fs::read_to_string("generated/middle.txt").unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn retargeted_outputs_stop() {
    struct Case {
        src: &'static str,
        child: &'static str,
        prepare: fn(&Path),
        retarget: fn(&Path),
        refused_as: &'static str,
        assert_preserved: fn(&Path, &str),
    }
    let cases = [
        Case {
            src: "[vendor]\npath = \"vendor\"\n\n[[hooks.before-build]]\nname = \"generator\"\nargv = [\"generator\"]\ngenerates = [\"alias/marker.txt\"]\n",
            child: "child_writes_declared_output",
            prepare: |root| fs::create_dir(root.join("alias")).unwrap(),
            retarget: |root| {
                fs::remove_dir(root.join("alias")).unwrap();
                let workspace = crate::filesystem::swap_workspace(&root.join("vendor")).unwrap();
                fs::create_dir(&workspace).unwrap();
                std::os::unix::fs::symlink(workspace, root.join("alias")).unwrap();
            },
            refused_as: "vendor workspace",
            assert_preserved: |root, _| assert!(!root.join("generated/site.css").exists()),
        },
        Case {
            src: "[[hooks.before-build]]\nname = \"generator\"\nargv = [\"generator\"]\ngenerates = [\"alias.toml\"]\n",
            child: "child_rewrites_the_configuration",
            prepare: |root| fs::write(root.join("alias.toml"), "generated").unwrap(),
            retarget: |root| {
                fs::remove_file(root.join("alias.toml")).unwrap();
                std::os::unix::fs::symlink(root.join("tola.toml"), root.join("alias.toml"))
                    .unwrap();
            },
            refused_as: "site configuration",
            assert_preserved: |root, src| {
                assert_eq!(fs::read_to_string(root.join("tola.toml")).unwrap(), src);
            },
        },
    ];

    for case in cases {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        (case.prepare)(root);
        let mut cfg = crate::config::tests::load_test_config(root, case.src);
        fs::create_dir_all(&cfg.build.publish_dir).unwrap();
        cfg.hooks.before_build[0].argv = Some(hook_child_command(&format!(
            "hooks::runner::tests::{}",
            case.child
        )));
        (case.retarget)(root);

        let err = run_before_build_hooks_with_cancellation(
            &cfg,
            BuildMode::Production,
            tola_input::InputScope::Online,
            &mut ReadyHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap_err();

        (case.assert_preserved)(root, case.src);
        let diags = tola_diagnostic::attached(&err.error).expect("the refused output is reported");
        assert_eq!(diags[0].code, tola_diagnostic::codes::build::HOOKS);
        assert!(diags[0].message.contains(case.refused_as), "{diags:?}");
    }
}

#[test]
fn child_rewrites_the_configuration() {
    if !is_hook_child_for("before-build") {
        return;
    }
    fs::write("alias.toml", "the hook rewrote the configuration").unwrap();
}

#[test]
fn child_writes_partial_then_fails() {
    if !is_hook_child_for("before-build") {
        return;
    }
    fs::create_dir_all("failed").unwrap();
    fs::write("failed/partial.txt", "partial").unwrap();
    panic!("generator failed after writing");
}

#[test]
fn child_records_before_build_start() {
    if !is_hook_child_for("before-build") {
        return;
    }
    crate::build::tests::assert_hook_environment("before-build");
    fs::write("before-started.txt", "started").unwrap();
}

#[test]
fn skipped_attempt_skips_hooks() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("content")).unwrap();
    fs::write(root.join("site.typ"), "#document(\"index.html\")[Body]").unwrap();
    let mut cfg = cfg_with_publish_dir(&dir);
    cfg.hooks
        .before_build
        .push(hook("child_records_before_build_start"));
    let marker = cfg.root().join("before-started.txt");

    let mut req = crate::build::BuildRequest::new(BuildMode::Production);
    req.hook_execution = HookExecution::Skip;
    crate::build::BuildSession::new()
        .prepare(Arc::new(cfg.clone()), req)
        .run()
        .unwrap();
    assert!(!marker.exists(), "a skipped attempt ran a hook command");

    let mut req = crate::build::BuildRequest::new(BuildMode::Production);
    req.hook_execution = HookExecution::Run;
    crate::build::BuildSession::new()
        .prepare(Arc::new(cfg), req)
        .run()
        .unwrap();
    assert!(marker.exists(), "the hook did not run under Run");
}

#[test]
fn participating_hooks_run_per_build() {
    let dir = TempDir::new().unwrap();
    let mut cfg = cfg_with_publish_dir(&dir);
    let counter = hook("child_counts_successful_runs");
    let mut disabled = hook("child_counts_successful_runs");
    disabled.enable = false;
    let mut prod_only = hook("child_writes_declared_output");
    prod_only.dev = crate::config::section::build::DevParticipation::Skip;
    prod_only.generates = vec!["generated/site.css".into()];
    cfg.hooks.before_build = vec![counter, disabled, prod_only];

    for expected in ["1", "2"] {
        let exec = run_before_build_hooks_with_cancellation(
            &cfg,
            BuildMode::Development,
            tola_input::InputScope::Online,
            &mut ReadyHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap();
        assert!(exec.hooks_executed());
        assert_eq!(
            fs::read_to_string(cfg.root().join("generated/count.txt")).unwrap(),
            expected
        );
        assert!(!cfg.root().join("generated/site.css").exists());
    }

    run_before_build_hooks_with_cancellation(
        &cfg,
        BuildMode::Production,
        tola_input::InputScope::Online,
        &mut ReadyHooks::default(),
        None,
        &mut Vec::new(),
        &HookExecutor::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(cfg.root().join("generated/count.txt")).unwrap(),
        "3"
    );
    assert_eq!(
        fs::read_to_string(cfg.root().join("generated/site.css")).unwrap(),
        "compiled"
    );
}

#[test]
fn failed_chain_restarts() {
    let dir = TempDir::new().unwrap();
    let mut cfg = cfg_with_publish_dir(&dir);
    let mut first = hook("child_counts_successful_runs");
    first.generates = vec!["generated/count.txt".into()];
    let mut middle = hook("child_writes_middle_chain_output");
    middle.order = 10;
    middle.generates = vec!["generated/middle.txt".into()];
    let mut last = hook("child_writes_final_chain_output");
    last.order = 20;
    last.generates = vec!["generated/final.txt".into()];
    cfg.hooks.before_build = vec![first, middle, last];
    let root = cfg.root();

    run_before_build_hooks_with_cancellation(
        &cfg,
        BuildMode::Development,
        tola_input::InputScope::Online,
        &mut ReadyHooks::default(),
        None,
        &mut Vec::new(),
        &HookExecutor::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("generated/final.txt")).unwrap(),
        "1"
    );
    fs::write(root.join("fail.txt"), "fail").unwrap();
    let failed = run_before_build_hooks_with_cancellation(
        &cfg,
        BuildMode::Development,
        tola_input::InputScope::Online,
        &mut ReadyHooks::default(),
        None,
        &mut Vec::new(),
        &HookExecutor::new(),
    )
    .unwrap_err();
    assert_eq!(
        fs::read_to_string(root.join("generated/count.txt")).unwrap(),
        "2"
    );
    assert_eq!(
        fs::read_to_string(root.join("generated/final.txt")).unwrap(),
        "1"
    );
    assert_eq!(
        failed
            .completed()
            .outputs()
            .iter()
            .map(HookOutput::logical_path)
            .collect::<Vec<_>>(),
        [root.join("generated/count.txt")]
    );
    let partial = failed.partial_outputs().unwrap();
    assert!(partial.is_current());
    assert!(partial.covers_path(&root.join("generated/middle.txt")));
    assert!(!partial.covers_path(&root.join("generated/final.txt")));

    fs::remove_file(root.join("fail.txt")).unwrap();
    let recovered = run_before_build_hooks_with_cancellation(
        &cfg,
        BuildMode::Development,
        tola_input::InputScope::Online,
        &mut ReadyHooks::default(),
        None,
        &mut Vec::new(),
        &HookExecutor::new(),
    )
    .unwrap()
    .into_source_hooks();
    for path in [
        "generated/count.txt",
        "generated/middle.txt",
        "generated/final.txt",
    ] {
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "3");
    }
    assert_eq!(recovered.outputs().len(), 3);
    assert!(recovered.outputs().iter().all(HookOutput::is_current));
}

#[test]
fn runner_panic_keeps_batch_evidence() {
    use crate::config::section::build::HookJobs;
    use crate::hooks::HookRunner;
    use crate::hooks::report::{CommandFailure, InterruptionStep};
    use std::num::NonZeroUsize;
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    use tola_diagnostic::{Diagnostic, Severity, codes};

    let dir = TempDir::new().unwrap();
    let (completed_peer, wait_for_peer) = mpsc::channel();
    let wait_for_peer = Mutex::new(wait_for_peer);
    let mut cfg = cfg_with_publish_dir(&dir).with_hook_runner(HookRunner::new(move |hook| {
        let root = hook.config.root();
        match hook.context.hook.as_ref().unwrap().name.as_str() {
            "partial" => {
                fs::write(root.join("generated/partial.txt"), "partial")?;
                wait_for_peer
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))?;
                panic!("private hook runner panic payload");
            }
            "complete" => {
                fs::write(root.join("generated/complete.txt"), "complete")?;
                Ok(vec![Diagnostic::new(
                    codes::hook::COMMAND,
                    Severity::Warning,
                    "The completed hook supplied this diagnostic",
                )])
            }
            "witness" => {
                // Two permits admit this member only after `complete` drops its permit.
                completed_peer.send(())?;
                Ok(Vec::new())
            }
            _ => unreachable!(),
        }
    }));
    let root = cfg.root().to_owned();
    fs::write(root.join("hook.nu"), "").unwrap();
    fs::create_dir(root.join("generated")).unwrap();
    cfg.hooks.before_build = [
        ("partial", vec!["generated/partial.txt".into()]),
        ("complete", vec!["generated/complete.txt".into()]),
        ("witness", Vec::new()),
    ]
    .into_iter()
    .map(|(name, generates)| BeforeBuildHookConfig {
        name: name.into(),
        nu: Some("hook.nu".into()),
        generates,
        ..Default::default()
    })
    .collect();

    let executor = HookExecutor::new();
    let cancel = BuildCancellation::new();
    executor
        .configure(
            executor.begin_attempt(),
            true,
            HookJobs::Limit(NonZeroUsize::new(2).unwrap()),
            &cancel,
        )
        .unwrap();
    let mut diags = Vec::new();
    let failure = run_before_build_hooks_with_cancellation(
        &cfg,
        BuildMode::Development,
        tola_input::InputScope::Online,
        &mut ReadyHooks::default(),
        Some(&cancel),
        &mut diags,
        &executor,
    )
    .unwrap_err();

    assert!(failure.error.chain().any(|cause| matches!(
        cause.downcast_ref::<CommandFailure>(),
        Some(CommandFailure::Interrupted {
            step: InterruptionStep::Run
        })
    )));
    assert!(!format!("{:#}", failure.error).contains("private hook runner panic payload"));
    assert_eq!(failure.completed().outputs().len(), 1);
    assert_eq!(
        failure.completed().outputs()[0].logical_path(),
        root.join("generated/complete.txt")
    );
    assert!(failure.completed().outputs()[0].is_current());
    let partial = failure.partial_outputs().unwrap();
    assert!(partial.is_current());
    assert!(partial.covers_path(&root.join("generated/partial.txt")));
    assert!(!partial.covers_path(&root.join("generated/complete.txt")));
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn child_waits_for_cancellation() {
    if !is_hook_child_for("before-build") {
        return;
    }
    crate::build::tests::record_hook_environment();
    fs::write("hook-ready.txt", "ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn hook_receives_build_mode() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let mut cfg = cfg_with_publish_dir(&dir);
    cfg.hooks.before_build.push(hook_named(
        "record",
        "build::tests::child_records_hook_environment",
    ));
    cfg.hooks
        .after_publish
        .push(crate::config::section::build::AfterPublishHookConfig {
            name: "record".into(),
            argv: Some(hook_child_command(
                "build::tests::child_records_hook_environment",
            )),
            dev: crate::config::section::build::DevParticipation::Run,
            ..Default::default()
        });

    for (mode, expected) in [
        (BuildMode::Development, "dev"),
        (BuildMode::Production, "prod"),
    ] {
        run_before_build_hooks_with_cancellation(
            &cfg,
            mode,
            tola_input::InputScope::Online,
            &mut ReadyHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap();
        assert_eq!(
            crate::build::tests::recorded_build_mode(&root),
            expected,
            "before-build {expected}"
        );
        run_after_publish_hooks(
            &cfg,
            mode,
            tola_input::InputScope::Online,
            &cfg.build.publish_dir,
            &BuildCancellation::new(),
            &HookExecutor::new(),
        )
        .unwrap();
        assert_eq!(
            crate::build::tests::recorded_build_mode(&root),
            expected,
            "after-publish {expected}"
        );
    }
}

#[test]
fn cache_survives_unfinished_run() {
    #[derive(Clone, Copy, Debug)]
    enum Ending {
        Failed,
        Cancelled,
    }
    for ending in [Ending::Failed, Ending::Cancelled] {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let mut cfg = cfg_with_publish_dir(&dir);
        cfg.hooks.before_build = vec![hook_named(
            "images",
            "build::tests::child_records_hook_environment",
        )];
        run_before_build_hooks_with_cancellation(
            &cfg,
            BuildMode::Production,
            tola_input::InputScope::Online,
            &mut ReadyHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap();
        let marker = crate::build::tests::recorded_hook_cache_directory(&root).join("marker.txt");
        fs::write(&marker, "kept").unwrap();

        match ending {
            Ending::Failed => {
                cfg.hooks.before_build = vec![hook_named(
                    "images",
                    "hooks::runner::tests::child_writes_partial_then_fails",
                )];
                let err = run_before_build_hooks_with_cancellation(
                    &cfg,
                    BuildMode::Production,
                    tola_input::InputScope::Online,
                    &mut ReadyHooks::default(),
                    None,
                    &mut Vec::new(),
                    &HookExecutor::new(),
                )
                .unwrap_err();
                assert!(
                    tola_diagnostic::attached(&err.error).is_some(),
                    "{ending:?}"
                );
            }
            Ending::Cancelled => {
                cfg.hooks.before_build = vec![hook_named(
                    "images",
                    "hooks::runner::tests::child_waits_for_cancellation",
                )];
                let canceller = crate::cancellation::BuildCanceller::new();
                let cancel = canceller.token();
                let err = std::thread::scope(|scope| {
                    let worker = scope.spawn(|| {
                        run_before_build_hooks_with_cancellation(
                            &cfg,
                            BuildMode::Development,
                            tola_input::InputScope::Online,
                            &mut ReadyHooks::default(),
                            Some(&cancel),
                            &mut Vec::new(),
                            &HookExecutor::new(),
                        )
                    });
                    let ready = root.join("hook-ready.txt");
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    while !ready.exists() {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "the hook command never started"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    canceller.cancel();
                    worker.join().unwrap()
                })
                .unwrap_err();
                assert!(
                    err.error.is::<crate::cancellation::BuildCancelled>(),
                    "{ending:?}"
                );
            }
        }

        assert_eq!(fs::read_to_string(&marker).unwrap(), "kept", "{ending:?}");
    }
}
