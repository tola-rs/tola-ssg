use std::sync::Arc;

use super::watches::DevFailureWatches;
use super::{BuildSession, DevBuildTrigger, RebuildLoop, RebuildRequest, RebuildStartup};
use crate::dev::watch::{FileChangeSource, RebuildScope, WatchError};

#[tokio::test]
async fn rejected_config_frees_hook_events() {
    for replacement in [
        "[build",
        "[site]\norigin = \"https://example.test\"\nbase-path = \"/moved/\"\n",
    ] {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::create_dir(root.join("generated")).unwrap();
        std::fs::write(root.join("site.typ"), "#document(\"index.html\")[Site]").unwrap();
        let generated = root.join("generated/input.txt");
        std::fs::write(&generated, "first").unwrap();
        let cfg_path = root.join("tola.toml");
        std::fs::write(
            &cfg_path,
            "[[hooks.before-build]]\nname = \"unused\"\n\
             argv = [\"unused-before-config-rejection\"]\ngenerates = [\"generated\"]\n",
        )
        .unwrap();
        let loaded = crate::config::load(
            Some(&cfg_path),
            tola_input::InputScope::Pure,
            tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
            &crate::config::ConfigOverrides::default(),
        )
        .unwrap();
        let (cfg, server, dev, config_loader) = loaded.into_parts();
        let cancellation = crate::cancellation::Cancellation::default();
        let output = crate::cli::output::CommandOutput::new(
            crate::terminal::Terminal::new(clap::ColorChoice::Never, false, None),
            None,
        );
        let build_session = BuildSession::with_resources(
            tola_build::BuildResources::new().with_input_scope(tola_input::InputScope::Pure),
        );
        let hooks = crate::dev::hooks::HookQueue::start(
            output.clone(),
            None,
            cancellation.clone(),
            crate::dev::events::Events(false),
            build_session.hook_executor().clone(),
        )
        .unwrap();
        let mut rebuilding = RebuildLoop::start(RebuildStartup {
            initial_config: Arc::clone(&cfg),
            server,
            dev,
            disable_hook_reuse: false,
            config_loader,
            build_session,
            sites: crate::dev::site::CurrentSite::new(),
            reload: None,
            shutdown: cancellation,
            output,
            hooks: hooks.sender(),
            events: crate::dev::events::Events(false),
        })
        .await
        .unwrap();
        std::fs::write(&cfg_path, replacement).unwrap();
        let rejected = rebuilding
            .settle_build(
                RebuildRequest {
                    paths: vec![cfg_path],
                    rebuild_scope: RebuildScope::FullSite,
                },
                DevBuildTrigger::FilesChanged,
            )
            .await;

        std::fs::write(&generated, "edited after config rejection").unwrap();
        let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let (paths, _) = rebuilding.changes.next(&cfg).await?.into_parts();
                if paths.iter().any(|path| generated.starts_with(path)) {
                    return Ok::<_, WatchError>(());
                }
            }
        })
        .await;
        let stopped = rebuilding.changes.shutdown().await;
        let hooks_stopped = hooks.finish();

        rejected.unwrap();
        observed.unwrap().unwrap();
        stopped.unwrap();
        hooks_stopped.unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn failed_font_reads_watched() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let site = root.join("site");
    std::fs::create_dir_all(site.join("content")).unwrap();
    std::fs::create_dir_all(site.join("fonts")).unwrap();
    let fonts = tola_typst::FontStore::with_options(
        tola_typst::FontOptions::new().with_system_fonts(false),
    );
    let font = fonts
        .load(&tola_typst::BundleCancellation::new())
        .unwrap()
        .font(0)
        .unwrap();
    let physical_font = root.join("outside.ttf");
    std::fs::write(&physical_font, font.data().as_slice()).unwrap();
    std::os::unix::fs::symlink(&physical_font, site.join("fonts/linked.ttf")).unwrap();
    let program = format!(
        "#set text(font: {:?})\n#document(\"index.html\")[#context {{ measure([Font]); panic(\"Failure after reading font\") }}]",
        font.info().family,
    );
    let settings = "[typst.fonts]\npaths = [\"fonts\"]\n";
    std::fs::write(site.join("tola.toml"), settings).unwrap();
    let cfg = crate::dev::tests::write_site(&site, &program, settings);
    let mut session = BuildSession::with_resources(tola_build::BuildResources::new());
    let failure = session
        .prepare(
            Arc::clone(&cfg),
            tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Development),
        )
        .run()
        .err()
        .expect("source fails after reading font");
    assert!(failure.input_observation().paths().iter().any(|path| {
        path.input() == tola_build::build::InputKind::FontInventory && path.path() == physical_font
    }));
    let mut watches = DevFailureWatches::default();
    watches.observe_failure(&failure);
    let dynamic = watches.requirements(&Default::default());
    let mut changes = FileChangeSource::start(&cfg, &dynamic, &Default::default(), None)
        .await
        .unwrap();
    let _ = changes.begin_candidate(&cfg);
    changes.reject_hook_candidate();
    std::fs::write(&physical_font, b"repaired font bytes").unwrap();
    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let (paths, _) = changes.next(&cfg).await?.into_parts();
            if paths.contains(&physical_font) {
                return Ok::<_, WatchError>(());
            }
        }
    })
    .await;
    changes.shutdown().await.unwrap();
    observed.unwrap().unwrap();

    for transition in ["replaced failure", "lost attempt", "published revision"] {
        watches.observe_failure(&failure);
        assert!(!watches.latest_failed_observed_paths.is_empty());
        match transition {
            "replaced failure" => watches.build_failed(std::iter::empty(), Vec::new()),
            "lost attempt" => watches.build_task_failed(&cfg),
            "published revision" => watches.current_site_replaced(),
            _ => unreachable!(),
        }
        assert!(
            watches.latest_failed_observed_paths.is_empty(),
            "{transition}"
        );
    }
}
