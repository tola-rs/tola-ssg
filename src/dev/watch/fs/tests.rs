use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use tola_build::build::InputPathScope;
use tola_build::config::SiteConfig;

use super::debouncer::{self, Debouncer};
use super::dispatch::{RebuildPathFilter, forward_watch_result, take_rebuild_batch};
use super::epoch::{ConfirmedHookPaths, EventEpoch, EventEpochStateGuard};
use super::observer::DirectoryObserver;
use super::producer::ProducerSet;
use super::queue::{WatchEventQueue, WatchMessage, WatchMessageSink};
use super::roots::RootSet;
use super::source::RunningHookPathCheck;
use super::{FileChangeBatch, FileChangeSource, ProducerKind, RebuildScope, WatchRequirements};

impl WatchMessageSink for mpsc::UnboundedSender<WatchMessage> {
    fn send_message(&self, msg: WatchMessage) -> Option<ProducerSet> {
        let _ = self.send(msg);
        None
    }
}

impl EventEpoch {
    fn guard_relevant(
        &self,
        expected: u64,
        relevant: impl FnMut(&Path) -> bool,
    ) -> Result<Option<EventEpochStateGuard<'_>>, Arc<notify::Error>> {
        self.guard_relevant_confirmed(expected, relevant, &ConfirmedHookPaths::default())
    }
}

#[test]
fn child_writes_nothing() {}

pub(super) fn resolve_schema(
    root: &Path,
    schema: tola_build::config::SiteConfigSchema,
    packages: tola_typst::PackageLocations,
) -> SiteConfig {
    std::fs::create_dir_all(root).unwrap();
    let path = root.join("tola.toml");
    if !path.exists() {
        std::fs::write(&path, toml::to_string(&schema).unwrap()).unwrap();
    }
    schema
        .resolve(
            &path,
            packages,
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
}

pub(super) fn local_packages() -> tola_typst::PackageLocations {
    tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap()
}

fn watch_config(root: &Path) -> SiteConfig {
    std::fs::create_dir_all(root.join("content")).unwrap();
    let entry = root.join("site.typ");
    if !entry.exists() {
        std::fs::write(&entry, "#document(\"index.html\")[Site]").unwrap();
    }
    resolve_schema(
        root,
        tola_build::config::SiteConfigSchema::default(),
        local_packages(),
    )
}

async fn manual_watch_source(
    cfg: &SiteConfig,
    source_hooks: &tola_build::hooks::SourceHookOutputs,
) -> FileChangeSource {
    let (event_tx, events) = WatchEventQueue::channel();
    let roots = RootSet::from_config(cfg, &WatchRequirements::default()).unwrap();
    let observer = DirectoryObserver::start(|| Ok(notify::NullWatcher))
        .await
        .unwrap();
    let mut src = FileChangeSource {
        event_tx,
        events,
        event_epoch: EventEpoch::for_root(cfg.root()),
        claimed_revision: 0,
        observer: Some(observer),
        roots,
        debouncer: Debouncer::new(),
        hook_candidate: None,
        source_hooks: source_hooks.clone(),
        failed_hook_outputs: None,
        hook_path_check: None,
    };
    src.install_selection(src.roots.initial_selection(), false)
        .await
        .unwrap();
    src.roots.confirm_current_observation();
    src
}

fn modified_event(path: impl Into<PathBuf>) -> notify::Event {
    notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Data(
        notify::event::DataChange::Any,
    )))
    .add_path(path.into())
}

fn watching_epoch(cfg: &SiteConfig, req: &WatchRequirements) -> EventEpoch {
    let epoch = EventEpoch::for_root(cfg.root());
    epoch.lock().boundary = RootSet::from_config(cfg, req)
        .unwrap()
        .initial_selection()
        .boundary();
    epoch
}

#[cfg(unix)]
fn deliver_event(src: &mut FileChangeSource, ev: &notify::Event) {
    forward_watch_result(Ok(ev.clone()), &src.event_epoch, &src.event_tx, None);
    src.debouncer.add_event(
        ev,
        std::time::Instant::now()
            - std::time::Duration::from_millis(debouncer::MAX_BATCH_WAIT_MS + 1),
    );
}

fn settled_batch(
    cfg: &SiteConfig,
    events: impl IntoIterator<Item = notify::Event>,
    rebuild_scope: RebuildScope,
) -> Option<FileChangeBatch> {
    let now = std::time::Instant::now();
    let mut debouncer = Debouncer::new();
    for ev in events {
        debouncer.add_event(&ev, now);
    }
    take_rebuild_batch(
        &mut debouncer,
        &RebuildPathFilter::for_config(cfg),
        rebuild_scope,
        now + std::time::Duration::from_millis(debouncer::QUIET_PERIOD_MS),
    )
}

#[tokio::test]
async fn shutdown_cancels_worker() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = watch_config(dir.path());
    let mut src = manual_watch_source(&cfg, &Default::default()).await;
    let canceller = tola_build::cancellation::BuildCanceller::new();
    let cancellation = canceller.token();
    let worker_cancellation = cancellation.clone();
    let (release, released) = std::sync::mpsc::channel();
    let (completed, completion) = std::sync::mpsc::channel();
    src.hook_path_check = Some(RunningHookPathCheck {
        canceller,
        worker: tokio::task::spawn_blocking(move || {
            released.recv().unwrap();
            let observed = worker_cancellation.ensure_active();
            completed.send(()).unwrap();
            observed?;
            Ok(Default::default())
        }),
    });
    let closing = src.shutdown();
    tokio::pin!(closing);
    assert!(
        std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(closing.as_mut(), cx).is_pending())
        })
        .await
    );
    assert!(cancellation.is_cancelled());

    release.send(()).unwrap();
    closing.await.unwrap();
    completion.try_recv().unwrap();
}

#[test]
fn callback_errors_forwarded() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = watch_config(dir.path());
    let boundary = RootSet::from_config(&cfg, &WatchRequirements::default())
        .unwrap()
        .initial_selection()
        .boundary();
    let expected_producers = boundary.producers();
    let epoch = EventEpoch::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    epoch.lock().boundary = boundary;

    forward_watch_result(
        Err(notify::Error::generic("injected callback failure")),
        &epoch,
        &tx,
        None,
    );

    let WatchMessage::Error { producers, error } = rx.try_recv().unwrap() else {
        panic!("expected error message");
    };
    assert_eq!(producers, expected_producers);
    assert_eq!(error.to_string(), "injected callback failure");
    assert_eq!(epoch.revision(), 1);
    assert!(epoch.guard_relevant(0, |_| true).is_err());
}

#[test]
fn log_file_excluded() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = watch_config(dir.path());
    let log_file = cfg.root().join("tola.log");
    let src = cfg.root().join("source.log");
    std::fs::write(&log_file, "session log").unwrap();
    std::fs::write(&src, "site input").unwrap();
    let log_file = std::fs::canonicalize(log_file).unwrap();
    let mut req = WatchRequirements::default();
    req.observe_paths(
        ProducerKind::TypstReads,
        InputPathScope::Recursive,
        true,
        [cfg.root().to_path_buf()],
    );
    let epoch = watching_epoch(&cfg, &req);
    let (tx, mut rx) = mpsc::unbounded_channel();

    forward_watch_result(Ok(modified_event(&log_file)), &epoch, &tx, Some(&log_file));
    assert_eq!(epoch.revision(), 0);
    assert!(rx.try_recv().is_err());
    #[cfg(unix)]
    {
        let alias = cfg.root().join("log-link");
        std::os::unix::fs::symlink(&log_file, &alias).unwrap();
        forward_watch_result(Ok(modified_event(alias)), &epoch, &tx, Some(&log_file));
        assert_eq!(epoch.revision(), 0);
        assert!(rx.try_recv().is_err());
    }

    forward_watch_result(
        Ok(modified_event(&log_file).add_path(src.clone())),
        &epoch,
        &tx,
        Some(&log_file),
    );
    let WatchMessage::Event { event, .. } = rx.try_recv().unwrap() else {
        panic!("expected event message");
    };
    assert_eq!(event.paths, [src]);

    forward_watch_result(
        Ok(modified_event(&log_file).set_flag(notify::event::Flag::Rescan)),
        &epoch,
        &tx,
        Some(&log_file),
    );
    let WatchMessage::Event { event, .. } = rx.try_recv().unwrap() else {
        panic!("expected rescan event");
    };
    assert!(event.need_rescan());
    assert!(epoch.requires_directory_refresh());
    assert_eq!(
        epoch.claim_candidate().rebuild_scope(),
        RebuildScope::FullSite
    );
}

#[test]
fn rescan_preserves_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = watch_config(dir.path());
    let root = cfg.root().to_path_buf();
    let epoch = watching_epoch(&cfg, &WatchRequirements::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let ev = notify::Event::new(notify::EventKind::Other)
        .add_path(root.clone())
        .set_flag(notify::event::Flag::Rescan)
        .set_info("rescan: user dropped");

    forward_watch_result(Ok(ev), &epoch, &tx, None);

    let WatchMessage::Event { event, .. } = rx.try_recv().unwrap() else {
        panic!("expected rescan event");
    };
    assert_eq!(event.paths, [root]);
    assert_eq!(event.info(), Some("rescan: user dropped"));
    assert!(event.need_rescan());
    assert!(epoch.requires_directory_refresh());
    assert!(epoch.guard_relevant(0, |_| true).unwrap().is_none());
}

#[test]
fn configured_tree_member() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let mut schema = tola_build::config::SiteConfigSchema::default();
    schema
        .assets
        .trees
        .push(tola_build::config::section::AssetTreeDeclaration::new(
            "assets",
            tola_build::config::section::AssetUrlPrefix::parse("/assets").unwrap(),
        ));
    let cfg = resolve_schema(root, schema, local_packages());
    std::fs::create_dir_all(cfg.root().join("assets")).unwrap();
    std::fs::create_dir_all(cfg.root().join("static/typst-fonts")).unwrap();
    let req = WatchRequirements::default();

    for (member, producer) in [
        (PathBuf::from("assets/data.tmp"), ProducerKind::SiteAssets),
        (
            PathBuf::from("static/typst-fonts/.#Nimbus.otf"),
            ProducerKind::Fonts,
        ),
    ] {
        let member = cfg.root().join(&member);
        let epoch = EventEpoch::default();
        epoch.lock().boundary = RootSet::from_config(&cfg, &req)
            .unwrap()
            .initial_selection()
            .boundary();
        let (tx, mut rx) = mpsc::unbounded_channel();

        forward_watch_result(Ok(modified_event(&member)), &epoch, &tx, None);

        let WatchMessage::Event {
            event, producers, ..
        } = rx.try_recv().unwrap()
        else {
            panic!("{} must stay observable", member.display());
        };
        assert_eq!(event.paths, [member]);
        assert_eq!(producers, ProducerSet::one(producer));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn hook_watch_tool_and_output() {
    use tola_build::build::{BuildMode, BuildRequest, BuildSession};
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("content")).unwrap();
    std::fs::write(root.join("site.typ"), "#document(\"index.html\")[Site]").unwrap();
    let module = root.join("module");
    std::fs::create_dir_all(module.join("tools")).unwrap();
    let tool = module.join("tools/input.txt");
    std::fs::write(&tool, "first").unwrap();
    std::fs::write(
        module.join("tola-module.toml"),
        "[[hooks.before-build]]\nname = \"generate\"\n\
         argv = [\"sh\", \"-c\", \"mkdir -p generated; printf generated > generated/notes.txt\"]\n\
         rerun-on = { include = [\"tools/*.txt\"] }\n\
         generates = [\"generated/{name}.txt\"]\n",
    )
    .unwrap();
    let cfg = tola_build::config::loading::resolve_site_config(
        &root.join("tola.toml"),
        "[modules.notes]\npath = \"module\"\n",
        local_packages(),
        &tola_build::config::loading::BuildOverrides::default(),
    )
    .unwrap()
    .into_config();
    let hook_watch = tola_build::modules::prepare_hook_watch(
        &cfg,
        tola_input::InputMode::new(tola_input::InputScope::Online, false),
        &Default::default(),
    )
    .unwrap();
    let mut changes = manual_watch_source(&cfg, &Default::default()).await;
    let claim = changes.begin_candidate(&cfg);
    changes.update_hook_candidate_claims(hook_watch.config());
    changes.extend_hook_candidate_claims(hook_watch.inputs());
    let mut dynamic = WatchRequirements::default();
    dynamic.add_module_hook_inputs(hook_watch.inputs());
    changes
        .stage_config(hook_watch.config(), &dynamic)
        .await
        .unwrap();
    changes.refresh_pending().await.unwrap();
    let mut session = BuildSession::new();
    let built = session
        .prepare_with_hook_watch(&hook_watch, BuildRequest::new(BuildMode::Development))
        .unwrap()
        .run()
        .unwrap();
    let output = root.join("generated/notes.txt");
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "generated");
    forward_watch_result(
        Ok(modified_event(&output)),
        &changes.event_epoch,
        &changes.event_tx,
        None,
    );
    let actual_cfg = Arc::clone(built.config());
    let source_hooks = built.source_hooks().clone();
    let subscriptions = changes
        .stage_config(
            &actual_cfg,
            &crate::dev::watch::watch_requirements_from_observation(&built.input_observation()),
        )
        .await
        .unwrap();
    assert!(changes.event_epoch.lock().pending.contains_key(&output));
    changes.refresh_pending().await.unwrap();
    assert!(changes.event_epoch.lock().pending.contains_key(&output));
    let guard = changes
        .guard_epoch(
            claim.revision(),
            &actual_cfg,
            source_hooks.outputs(),
            &tola_build::cancellation::BuildCancellation::default(),
        )
        .await
        .unwrap()
        .expect("confirmed generator output must not reject candidate");
    guard.commit(claim, subscriptions);
    changes.complete_hook_candidate(&source_hooks);
    assert!(changes.event_epoch.claim_candidate().paths.is_empty());

    std::fs::write(&tool, "changed").unwrap();
    let ev = modified_event(&tool);
    deliver_event(&mut changes, &ev);
    let batch = tokio::time::timeout(std::time::Duration::from_secs(1), changes.next(&actual_cfg))
        .await
        .unwrap()
        .unwrap();
    assert!(batch.paths.contains(&tool), "{batch:?}");
    changes.shutdown().await.unwrap();
}

#[test]
fn internal_events_ignored() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = resolve_schema(
        dir.path(),
        toml::from_str(
            "[[hooks.before-build]]\nname = \"inputs\"\nargv = [\"unused\"]\n\
             rerun-on = { include = [\"**\"] }\n",
        )
        .unwrap(),
        local_packages(),
    );
    let root = cfg.root();
    let epoch = watching_epoch(&cfg, &WatchRequirements::default());
    let rev = epoch.revision();
    let (tx, mut rx) = mpsc::unbounded_channel();
    for ev in [
        modified_event(root.join(".tola-build.lock")),
        notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::From,
        )))
        .add_path(root.join(".tola/cache")),
    ] {
        forward_watch_result(Ok(ev), &epoch, &tx, None);
    }
    assert!(rx.try_recv().is_err());
    assert!(epoch.guard_relevant(rev, |_| true).unwrap().is_some());

    forward_watch_result(
        Ok(modified_event(root.join("input.typ"))),
        &epoch,
        &tx,
        None,
    );
    assert!(rx.try_recv().is_ok());
    assert!(epoch.guard_relevant(rev, |_| true).unwrap().is_none());
}

#[test]
fn generated_output_ignored() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = watch_config(dir.path());
    let root = cfg.root().to_path_buf();
    assert!(
        settled_batch(
            &cfg,
            [
                modified_event(root.join("public/index.html")),
                modified_event(root.join(".tola-build.lock")),
                modified_event(root.join(".public-swap/staged/0/index.html")),
            ],
            RebuildScope::Paths,
        )
        .is_none()
    );
    let batch = settled_batch(
        &cfg,
        [
            modified_event(root.join("public/index.html")),
            modified_event(root.join("content/index.typ")),
        ],
        RebuildScope::Paths,
    )
    .unwrap();
    assert_eq!(batch.paths, vec![root.join("content/index.typ")]);
}

#[tokio::test]
async fn directory_rename_exclusions() {
    use notify::event::{ModifyKind, RenameMode};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("generated/brand")).unwrap();
    std::fs::create_dir(root.join("scratch")).unwrap();
    let cfg = resolve_schema(
        &root,
        toml::from_str(
            "[[hooks.before-build]]\nname = \"styles\"\nargv = [\"unused\"]\n\
             rerun-on = { include = [\"**/*.typ\"], exclude = [\"scratch/**\"] }\n\
             generates = [\"generated/brand\"]\n",
        )
        .unwrap(),
        local_packages(),
    );
    let mut changes = manual_watch_source(&cfg, &Default::default()).await;
    changes.begin_candidate(&cfg);
    let rename = |from: &str, to: &str| {
        notify::Event::new(notify::EventKind::Modify(ModifyKind::Name(
            RenameMode::Both,
        )))
        .add_path(root.join(from))
        .add_path(root.join(to))
    };
    for ev in [
        rename("scratch/previous", "scratch/older"),
        rename("generated/brand", "scratch/previous"),
    ] {
        forward_watch_result(Ok(ev), &changes.event_epoch, &changes.event_tx, None);
    }
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(debouncer::MAX_BATCH_WAIT_MS + 1),
            changes.next(&cfg),
        )
        .await
        .is_err()
    );

    forward_watch_result(
        Ok(rename("chapters/old", "chapters/new")),
        &changes.event_epoch,
        &changes.event_tx,
        None,
    );
    let batch = tokio::time::timeout(std::time::Duration::from_secs(2), changes.next(&cfg))
        .await
        .unwrap()
        .unwrap();
    assert!(batch.paths.contains(&root.join("chapters/new")));
    changes.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_names_retained_hook() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let module = root.join("theme");
    std::fs::create_dir(&module).unwrap();
    let manifest = module.join("tola-module.toml");
    std::fs::write(
        &manifest,
        "[[hooks.before-build]]\nname = \"styles\"\nargv = [\"unused\"]\n\
         rerun-on = { include = [{ project = \"**/*.typ\" }] }\n",
    )
    .unwrap();
    let cfg = resolve_schema(
        &root,
        toml::from_str("[modules.theme]\npath = \"theme\"\n").unwrap(),
        local_packages(),
    );
    let prepared = tola_build::modules::prepare_hook_watch(
        &cfg,
        tola_input::InputMode::new(tola_input::InputScope::Pure, false),
        &Default::default(),
    )
    .unwrap();
    assert!(prepared.config().hooks().before_build.is_empty());
    let mut dynamic = WatchRequirements::default();
    dynamic.add_module_hook_inputs(prepared.inputs());
    let mut changes = manual_watch_source(&cfg, &Default::default()).await;
    changes
        .stage_config(prepared.config(), &dynamic)
        .await
        .unwrap();
    changes
        .stage_config(&cfg, &Default::default())
        .await
        .unwrap();
    let path = root.join("generated/previous");
    let ev = notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Name(
        notify::event::RenameMode::From,
    )))
    .add_path(path.clone());
    forward_watch_result(Ok(ev), &changes.event_epoch, &changes.event_tx, None);

    let watches = changes.hook_watches(&[path]);
    assert_eq!(watches.len(), 1);
    assert_eq!(watches[0].name, "styles");
    assert_eq!(watches[0].file, manifest);
    changes.shutdown().await.unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_alias_excludes_outputs() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let physical_root = dir.path().join("site");
    let alias_root = dir.path().join("site-alias");
    std::fs::create_dir(&physical_root).unwrap();
    symlink(&physical_root, &alias_root).unwrap();

    let cfg = watch_config(&alias_root);
    let filter = RebuildPathFilter::for_config(&cfg);
    let mirror = tola_input::layout::builtin_packages(&alias_root);
    assert!(!filter.accepts(&alias_root.join("public/index.html")));
    assert!(!filter.accepts(&mirror.join("cache")));
    assert!(!filter.accepts(&alias_root.join(".tola-build.lock")));
    assert!(!filter.accepts(&alias_root.join(".public-swap/previous/0/index.html")));
    let packages =
        tola_typst::PackageLocations::from_absolute_roots(None, Some(mirror.clone())).unwrap();
    let cfg = resolve_schema(
        &alias_root,
        tola_build::config::SiteConfigSchema::default(),
        packages,
    );
    let filter = RebuildPathFilter::for_config(&cfg);
    assert!(!filter.accepts(&mirror.join("cache")));
    assert!(filter.accepts(&alias_root.join("content/index.typ")));
}
