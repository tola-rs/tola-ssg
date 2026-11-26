use super::*;
use crate::build::tests::{hook_child_command, is_hook_child_for};
use crate::config::section::build::hooks::{DevParticipation, OutputCommandConfig};
use crate::output::graph::{OutputGraphBuilder, OutputKind};
use crate::output::semantics::{DeclaredOutputSemantics, ResponseMediaType};
use std::io::{Read, Write};
use std::sync::mpsc;

fn generated_root() -> PathBuf {
    crate::build::tests::hook_context()
        .unwrap()
        .hook
        .unwrap()
        .output_dir
        .unwrap()
}

fn output_cmd_cfg(
    child: &str,
    outputs: &[&str],
) -> (tempfile::TempDir, SiteConfig, OutputGraphBuilder) {
    let dir = tempfile::TempDir::new().unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    cfg.hooks.generate_outputs = vec![OutputCommandConfig {
        name: "search".into(),
        argv: Some(hook_child_command(&format!(
            "hooks::generate::tests::{child}"
        ))),
        outputs: outputs
            .iter()
            .map(|p| CommandOutput::File(OutputPath::parse(p).unwrap()))
            .collect(),
        ..OutputCommandConfig::default()
    }];
    let mut upstream = OutputGraphBuilder::new();
    upstream
        .insert(OutputFile::new(
            OutputPath::parse("index.html").unwrap(),
            OutputDeclaration::html_document(),
            OutputOwner::bundle("site.typ"),
            b"<h1>Home</h1>".to_vec(),
        ))
        .unwrap();
    (dir, cfg, upstream)
}

#[test]
fn child_writes_search_index() {
    if !is_hook_child_for("generate-outputs") {
        return;
    }
    crate::build::tests::assert_hook_environment("generate-outputs");
    let candidate = crate::build::tests::hook_context()
        .unwrap()
        .hook
        .unwrap()
        .input_dir
        .unwrap();
    let html = fs::read_to_string(candidate.join("index.html")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(candidate.join("index.html"))
                .unwrap()
                .permissions()
                .mode()
                & 0o222,
            0
        );
    }
    fs::write(
        generated_root().join("search.json"),
        serde_json::to_vec(&serde_json::json!({"html": html})).unwrap(),
    )
    .unwrap();
}

#[test]
fn child_counts_runs() {
    if !is_hook_child_for("generate-outputs") {
        return;
    }
    let path = Path::new("runs.txt");
    let count = fs::read_to_string(path)
        .ok()
        .and_then(|runs| runs.trim().parse::<u32>().ok())
        .unwrap_or(0);
    fs::write(path, (count + 1).to_string()).unwrap();
    let candidate = crate::build::tests::hook_context()
        .unwrap()
        .hook
        .unwrap()
        .input_dir
        .unwrap();
    let html = fs::read_to_string(candidate.join("index.html")).unwrap();
    fs::write(
        generated_root().join("search.json"),
        serde_json::to_vec(&serde_json::json!({"html": html})).unwrap(),
    )
    .unwrap();
}

#[test]
fn child_restores_cached_search_index() {
    if !is_hook_child_for("generate-outputs") {
        return;
    }
    let cache = crate::build::tests::hook_context()
        .unwrap()
        .hook
        .unwrap()
        .cache_dir
        .unwrap();
    let cached = cache.join("search/index.json");
    let target = generated_root().join("search/index.json");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    match fs::read(&cached) {
        Ok(bytes) => {
            assert!(!target.exists(), "Tola reused a generated root");
            fs::write(target, bytes).unwrap();
        }
        Err(_) => {
            let candidate = crate::build::tests::hook_context()
                .unwrap()
                .hook
                .unwrap()
                .input_dir
                .unwrap();
            let html = fs::read_to_string(candidate.join("index.html")).unwrap();
            let bytes = serde_json::to_vec(&serde_json::json!({"html": html})).unwrap();
            fs::create_dir_all(cached.parent().unwrap()).unwrap();
            fs::write(&cached, &bytes).unwrap();
            fs::write(target, bytes).unwrap();
        }
    }
}

#[test]
fn child_writes_extra_file() {
    if !is_hook_child_for("generate-outputs") {
        return;
    }
    fs::write(generated_root().join("search.json"), "[]").unwrap();
    fs::write(generated_root().join("extra.json"), "[]").unwrap();
}

#[test]
fn child_writes_nothing() {}

#[cfg(unix)]
#[test]
fn child_writes_symlink() {
    if !is_hook_child_for("generate-outputs") {
        return;
    }
    let candidate = crate::build::tests::hook_context()
        .unwrap()
        .hook
        .unwrap()
        .input_dir
        .unwrap();
    let out = generated_root();
    if Path::new("replace-output-root").exists() {
        fs::remove_dir(&out).unwrap();
        std::os::unix::fs::symlink(candidate, out).unwrap();
    } else {
        std::os::unix::fs::symlink(candidate.join("index.html"), out.join("search.json")).unwrap();
    }
}

#[test]
fn child_waits_for_cancellation() {
    if !is_hook_child_for("generate-outputs") {
        return;
    }
    let addr = fs::read_to_string("signal.txt").unwrap();
    let mut conn = std::net::TcpStream::connect(addr).unwrap();
    conn.write_all(b"R").unwrap();
    let mut byte = [0];
    let _ = conn.read_exact(&mut byte);
}

#[test]
fn generated_outputs_join_graph() {
    let (_dir, mut cfg, mut upstream) =
        output_cmd_cfg("child_writes_search_index", &["search.json"]);
    let mut disabled = cfg.hooks.generate_outputs[0].clone();
    disabled.enable = false;
    cfg.hooks.generate_outputs.insert(0, disabled);
    let outputs = generate_outputs(
        &cfg,
        upstream.outputs(),
        BuildMode::Production,
        tola_input::InputScope::Online,
        None,
        &BuildCancellation::new(),
        &mut Vec::new(),
        &HookExecutor::new(),
    )
    .unwrap();
    assert_eq!(outputs.outputs.len(), 1);
    let generated = &outputs.outputs[0];
    assert_eq!(generated.path().as_str(), "search.json");
    assert_eq!(generated.kind(), OutputKind::Asset);
    assert_eq!(
        generated.declaration().semantics(),
        DeclaredOutputSemantics::Opaque
    );
    assert_eq!(
        generated.declaration().media_type(),
        &ResponseMediaType::JSON
    );
    assert!(
        matches!(generated.owner(), OutputOwner::Command { index: 1, name } if name.as_ref() == "search")
    );
    let content: serde_json::Value = serde_json::from_slice(generated.bytes()).unwrap();
    assert_eq!(content["html"], "<h1>Home</h1>");
    assert!(!cfg.build.publish_dir.exists());
    upstream
        .insert(outputs.outputs.into_iter().next().unwrap())
        .unwrap();
    assert_eq!(upstream.finish().outputs().len(), 2);
}

#[test]
fn output_declarations_enforced() {
    for (child, declared) in [
        ("child_writes_nothing", vec!["search.json"]),
        ("child_writes_extra_file", vec!["search.json"]),
        ("child_writes_nothing", vec![]),
    ] {
        let (_dir, cfg, upstream) = output_cmd_cfg(child, &declared);
        let err = generate_outputs(
            &cfg,
            upstream.outputs(),
            BuildMode::Production,
            tola_input::InputScope::Online,
            None,
            &BuildCancellation::new(),
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap_err();
        assert!(tola_diagnostic::attached(&err).is_some());
        assert_eq!(upstream.outputs()[0].bytes(), b"<h1>Home</h1>");
    }
}

#[cfg(unix)]
#[test]
fn generated_symlinks_rejected() {
    for (replace_root, declared) in [(false, "search.json"), (true, "index.html")] {
        let (dir, cfg, upstream) = output_cmd_cfg("child_writes_symlink", &[declared]);
        if replace_root {
            fs::write(dir.path().join("replace-output-root"), "").unwrap();
        }
        let err = generate_outputs(
            &cfg,
            upstream.outputs(),
            BuildMode::Production,
            tola_input::InputScope::Online,
            None,
            &BuildCancellation::new(),
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap_err();
        assert_eq!(
            tola_diagnostic::attached(&err).unwrap()[0].code,
            tola_diagnostic::codes::build::HOOKS
        );
        assert_eq!(upstream.outputs()[0].bytes(), b"<h1>Home</h1>");
    }
}

#[test]
fn cancellation_stops_command() {
    let (dir, cfg, upstream) = output_cmd_cfg("child_waits_for_cancellation", &["search.json"]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    fs::write(dir.path().join("signal.txt"), addr.to_string()).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let observer = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        conn.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut ready = [0];
        let received = conn.read_exact(&mut ready).map(|()| ready);
        let _ = ready_tx.send(received);
        let _ = release_rx.recv();
    });
    let canceller = crate::cancellation::BuildCanceller::new();
    let worker_cancel = canceller.token();
    let worker = std::thread::spawn(move || {
        generate_outputs(
            &cfg,
            upstream.outputs(),
            BuildMode::Production,
            tola_input::InputScope::Online,
            None,
            &worker_cancel,
            &mut Vec::new(),
            &HookExecutor::new(),
        )
    });
    let ready = ready_rx.recv_timeout(std::time::Duration::from_secs(5));
    canceller.cancel();
    if ready.is_err() {
        let _ = std::net::TcpStream::connect(addr).and_then(|mut c| c.write_all(b"W"));
    }
    let completion = worker.join().unwrap();
    let _ = release_tx.send(());
    observer.join().unwrap();
    assert_eq!(ready.unwrap().unwrap(), [b'R']);
    let err = completion.unwrap_err();
    assert!(crate::cancellation::is_cancelled(&err), "{err:#}");
    assert!(tola_diagnostic::attached(&err).is_none());
}

#[test]
fn cached_tree_restores_fresh_output() {
    let (_dir, mut cfg, upstream) = output_cmd_cfg("child_restores_cached_search_index", &[]);
    cfg.hooks.generate_outputs[0].outputs =
        vec![CommandOutput::Tree(OutputPath::parse("search").unwrap())];

    for _ in 0..2 {
        let generated = generate_outputs(
            &cfg,
            upstream.outputs(),
            BuildMode::Production,
            tola_input::InputScope::Online,
            None,
            &BuildCancellation::new(),
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap();
        assert_eq!(generated.root_ownerships[0].root().as_str(), "search");
        assert_eq!(generated.outputs[0].path().as_str(), "search/index.json");
    }
}

fn run_count(dir: &tempfile::TempDir) -> u32 {
    fs::read_to_string(dir.path().join("runs.txt"))
        .ok()
        .and_then(|runs| runs.trim().parse().ok())
        .unwrap_or(0)
}

#[test]
fn reuse_policy_decides_run() {
    struct Case {
        mode: BuildMode,
        dev: DevParticipation,
        changes_upstream: bool,
        expected_runs: u32,
    }
    for case in [
        Case {
            mode: BuildMode::Development,
            dev: DevParticipation::Reuse,
            changes_upstream: false,
            expected_runs: 1,
        },
        Case {
            mode: BuildMode::Development,
            dev: DevParticipation::Reuse,
            changes_upstream: true,
            expected_runs: 2,
        },
        Case {
            mode: BuildMode::Development,
            dev: DevParticipation::Run,
            changes_upstream: false,
            expected_runs: 2,
        },
        Case {
            mode: BuildMode::Production,
            dev: DevParticipation::Reuse,
            changes_upstream: false,
            expected_runs: 2,
        },
    ] {
        let (dir, mut cfg, upstream) = output_cmd_cfg("child_counts_runs", &["search.json"]);
        cfg.hooks.generate_outputs[0].dev = case.dev;
        let cancel = BuildCancellation::new();
        let first = generate_outputs(
            &cfg,
            upstream.outputs(),
            case.mode,
            tola_input::InputScope::Online,
            None,
            &cancel,
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap();
        assert_eq!(run_count(&dir), 1);

        let html = if case.changes_upstream {
            b"<h1>Changed</h1>".as_slice()
        } else {
            b"<h1>Home</h1>".as_slice()
        };
        let mut next = OutputGraphBuilder::new();
        next.insert(OutputFile::new(
            OutputPath::parse("index.html").unwrap(),
            OutputDeclaration::html_document(),
            OutputOwner::bundle("site.typ"),
            html.to_vec(),
        ))
        .unwrap();
        let second = generate_outputs(
            &cfg,
            next.outputs(),
            case.mode,
            tola_input::InputScope::Online,
            Some(&first.reuse),
            &cancel,
            &mut Vec::new(),
            &HookExecutor::new(),
        )
        .unwrap();

        let label = format!(
            "{:?}/{:?}/changed={}",
            case.mode, case.dev, case.changes_upstream
        );
        assert_eq!(run_count(&dir), case.expected_runs, "{label}");
        if case.expected_runs == 1 {
            assert_eq!(
                first.outputs[0].bytes(),
                second.outputs[0].bytes(),
                "{label}"
            );
        }
    }
}
