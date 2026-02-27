use std::path::PathBuf;
use std::time::Duration;

use rustc_hash::FxHashMap;
use tempfile::TempDir;

use super::classifier::EventClassifier;
use super::debouncer::{DEBOUNCE_MS, Debouncer, REBUILD_COOLDOWN_MS};
use super::router::events_to_messages;
use super::types::{ChangeKind, DebouncedEvents};
use crate::actor::messages::CompilerMsg;
use crate::config::SiteConfig;
use crate::config::section::build::{HookConfig, WatchMode};
use crate::utils::path::normalize_path;

fn make_config() -> (TempDir, SiteConfig) {
    let temp = TempDir::new().unwrap();
    let root = normalize_path(temp.path());

    let mut config = SiteConfig::default();
    config.set_root(&root);
    config.build.content = root.join("content");
    config.build.output = root.join("public");
    config.config_path = root.join("tola.toml");

    std::fs::create_dir_all(&config.build.content).unwrap();
    std::fs::create_dir_all(config.paths().output_dir()).unwrap();

    (temp, config)
}

fn make_event(paths: Vec<&str>, kind: notify::EventKind) -> notify::Event {
    notify::Event {
        kind,
        paths: paths.into_iter().map(PathBuf::from).collect(),
        attrs: Default::default(),
    }
}

fn modify_kind() -> notify::EventKind {
    notify::EventKind::Modify(notify::event::ModifyKind::Data(
        notify::event::DataChange::Any,
    ))
}

fn create_kind() -> notify::EventKind {
    notify::EventKind::Create(notify::event::CreateKind::File)
}

fn remove_kind() -> notify::EventKind {
    notify::EventKind::Remove(notify::event::RemoveKind::File)
}

#[test]
fn test_debouncer_empty() {
    let debouncer = Debouncer::new();
    assert!(!debouncer.is_ready());
}

#[test]
fn test_event_routing_by_kind() {
    let mut debouncer = Debouncer::new();

    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
    debouncer.add_event(&make_event(vec!["/tmp/b.typ"], modify_kind()));
    debouncer.add_event(&make_event(vec!["/tmp/c.typ"], remove_kind()));

    assert_eq!(debouncer.changes.len(), 3);
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/a.typ")],
        ChangeKind::Created
    );
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/b.typ")],
        ChangeKind::Modified
    );
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/c.typ")],
        ChangeKind::Removed
    );
}

#[test]
fn test_temp_file_ignored() {
    let mut debouncer = Debouncer::new();

    debouncer.add_event(&make_event(vec!["/tmp/real.typ"], modify_kind()));
    assert!(debouncer.last_event.is_some());
    let first_time = debouncer.last_event.unwrap();

    std::thread::sleep(Duration::from_millis(5));

    // Temp file event — should NOT update last_event or add to changes
    debouncer.add_event(&make_event(vec!["/tmp/.swp"], modify_kind()));
    assert_eq!(debouncer.last_event.unwrap(), first_time);
    assert_eq!(debouncer.changes.len(), 1);
}

#[test]
fn test_dedup_first_event_wins() {
    let mut debouncer = Debouncer::new();

    // Same path: create then modify — first one (create) wins
    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], modify_kind()));

    assert_eq!(debouncer.changes.len(), 1);
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/a.typ")],
        ChangeKind::Created
    );
}

#[test]
fn test_dedup_same_event() {
    let mut debouncer = Debouncer::new();
    debouncer.add_event(&make_event(vec!["/tmp/a.typ", "/tmp/a.typ"], modify_kind()));
    assert_eq!(debouncer.changes.len(), 1);
}

#[test]
fn test_sleep_duration_no_events() {
    let debouncer = Debouncer::new();
    assert!(debouncer.sleep_duration() >= Duration::from_secs(3600));
}

#[test]
fn test_sleep_duration_after_event() {
    let mut debouncer = Debouncer::new();
    debouncer.last_event = Some(std::time::Instant::now());

    let dur = debouncer.sleep_duration();
    assert!(dur >= Duration::from_millis(DEBOUNCE_MS - 10));
    assert!(dur <= Duration::from_millis(DEBOUNCE_MS + 10));
}

#[test]
fn test_sleep_duration_respects_cooldown() {
    let mut debouncer = Debouncer::new();
    debouncer.last_event = Some(std::time::Instant::now());
    debouncer.last_compile = Some(std::time::Instant::now());

    let dur = debouncer.sleep_duration();
    assert!(dur >= Duration::from_millis(REBUILD_COOLDOWN_MS - 10));
    assert!(dur <= Duration::from_millis(REBUILD_COOLDOWN_MS + 10));
}

#[test]
fn test_debounced_events_split() {
    let events = DebouncedEvents(vec![
        (PathBuf::from("/a.typ"), ChangeKind::Created),
        (PathBuf::from("/b.typ"), ChangeKind::Modified),
        (PathBuf::from("/c.typ"), ChangeKind::Removed),
        (PathBuf::from("/d.typ"), ChangeKind::Created),
    ]);

    let (created, modified, removed) = events.split();
    assert_eq!(created.len(), 2);
    assert_eq!(modified.len(), 1);
    assert_eq!(removed.len(), 1);
}

#[test]
fn test_remove_then_create_restores() {
    let mut debouncer = Debouncer::new();

    // File removed, then restored (created) — should become Created
    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], remove_kind()));
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/a.typ")],
        ChangeKind::Removed
    );

    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
    assert_eq!(debouncer.changes.len(), 1);
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/a.typ")],
        ChangeKind::Created
    );
}

#[test]
fn test_create_then_remove_discards() {
    let mut debouncer = Debouncer::new();

    // File created, then removed — net no-op, should be discarded entirely
    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/a.typ")],
        ChangeKind::Created
    );

    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], remove_kind()));
    assert!(
        debouncer.changes.is_empty(),
        "created+removed should discard"
    );
}

#[test]
fn test_modify_then_remove_upgrades() {
    let mut debouncer = Debouncer::new();

    // File modified, then removed — should upgrade to Removed
    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], modify_kind()));
    debouncer.add_event(&make_event(vec!["/tmp/a.typ"], remove_kind()));
    assert_eq!(debouncer.changes.len(), 1);
    assert_eq!(
        debouncer.changes[&PathBuf::from("/tmp/a.typ")],
        ChangeKind::Removed
    );
}

#[test]
fn test_events_to_messages_includes_output_change() {
    let (_tmp, config) = make_config();
    let output_css = config.paths().output_dir().join("assets").join("hook.css");
    let output_html = config.paths().output_dir().join("page").join("index.html");
    std::fs::create_dir_all(output_css.parent().unwrap()).unwrap();
    std::fs::create_dir_all(output_html.parent().unwrap()).unwrap();
    std::fs::write(&output_css, "body{}").unwrap();
    std::fs::write(&output_html, "<html></html>").unwrap();

    let events = DebouncedEvents(vec![
        (output_css.clone(), ChangeKind::Modified),
        (output_html.clone(), ChangeKind::Created),
    ]);

    let messages = events_to_messages(events, &config);
    let output_msg = messages.into_iter().find_map(|msg| match msg {
        CompilerMsg::OutputChange(paths) => Some(paths),
        _ => None,
    });

    let paths = output_msg.expect("expected OutputChange message");
    assert!(paths.contains(&output_css));
    assert!(paths.contains(&output_html));
}

#[test]
fn test_filter_actionable_keeps_removed_output() {
    let (_tmp, config) = make_config();
    let removed_output = config
        .paths()
        .output_dir()
        .join("assets")
        .join("removed.css");
    let mut changes = FxHashMap::default();
    changes.insert(removed_output.clone(), ChangeKind::Removed);

    EventClassifier::filter_actionable(&mut changes, &config);
    assert!(changes.contains_key(&removed_output));
}

#[test]
fn test_asset_change_without_watched_hooks_does_not_enqueue_compile() {
    let (_tmp, mut config) = make_config();
    let root = config.get_root().to_path_buf();
    config.build.assets.normalize(&root);
    let asset = config.get_root().join("assets/styles/tailwind.css");
    let events = DebouncedEvents(vec![(asset, ChangeKind::Modified)]);

    let messages = events_to_messages(events, &config);
    assert!(messages
        .iter()
        .any(|m| matches!(m, CompilerMsg::AssetChange(_))));
    assert!(!messages
        .iter()
        .any(|m| matches!(m, CompilerMsg::Compile { .. })));
}

#[test]
fn test_asset_change_with_watched_hooks_enqueues_hook_only_compile() {
    let (_tmp, mut config) = make_config();
    let root = config.get_root().to_path_buf();
    config.build.assets.normalize(&root);
    config.build.hooks.pre.push(HookConfig {
        enable: true,
        name: Some("watched".into()),
        command: vec!["echo".into(), "hook".into()],
        watch: WatchMode::Bool(true),
        build_args: vec![],
        quiet: true,
    });

    let asset = config.get_root().join("assets/styles/tailwind.css");
    let events = DebouncedEvents(vec![(asset, ChangeKind::Modified)]);
    let messages = events_to_messages(events, &config);

    assert!(messages
        .iter()
        .any(|m| matches!(m, CompilerMsg::AssetChange(_))));

    let compile = messages.into_iter().find_map(|m| match m {
        CompilerMsg::Compile {
            queue,
            changed_paths,
        } => Some((queue, changed_paths)),
        _ => None,
    });
    let (queue, changed_paths) = compile.expect("expected hook-only compile message");
    assert!(queue.is_empty());
    assert_eq!(changed_paths.len(), 1);
}
