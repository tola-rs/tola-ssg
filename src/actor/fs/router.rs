use std::path::{Path, PathBuf};

use rustc_hash::FxHashSet;

use super::types::DebouncedEvents;
use crate::actor::messages::CompilerMsg;
use crate::config::SiteConfig;
use crate::reload::classify::classify_changes;

pub(super) fn log_events(events: &DebouncedEvents) {
    crate::debug_do! {
        for (path, kind) in &events.0 {
            crate::log!("watch"; "{}: {}", kind.label(), path.display());
        }
    }
}

/// Convert DebouncedEvents to CompilerMsg(s)
pub(super) fn events_to_messages(events: DebouncedEvents, config: &SiteConfig) -> Vec<CompilerMsg> {
    use crate::reload::classify::{FileCategory, categorize_path};

    let (created, modified, removed) = events.split();

    let mut classify_paths = modified;
    classify_paths.extend(
        created
            .iter()
            .chain(removed.iter())
            .filter(|p| {
                matches!(
                    categorize_path(p, config),
                    FileCategory::Deps | FileCategory::Asset
                )
            })
            .cloned(),
    );

    // Classify modified files plus created/removed deps and assets through the existing pipeline.
    let result = classify_changes(&classify_paths, config);

    if result.config_changed {
        return vec![CompilerMsg::FullRebuild];
    }

    let mut messages = Vec::new();

    // Process removed files FIRST (before created) to handle renames correctly
    // This ensures old state is cleaned up before new files are compiled
    let removed_content: Vec<_> = removed
        .iter()
        .filter(|p| matches!(categorize_path(p, config), FileCategory::Content(_)))
        .cloned()
        .collect();
    if !removed_content.is_empty() {
        messages.push(CompilerMsg::ContentRemoved(removed_content));
    }

    // Filter created files to content files in content directory
    let created_content: Vec<_> = created
        .iter()
        .filter(|p| matches!(categorize_path(p, config), FileCategory::Content(_)))
        .cloned()
        .collect();
    if !created_content.is_empty() {
        messages.push(CompilerMsg::ContentCreated(created_content));
    }

    let changed_paths: Vec<PathBuf> = result.classified.iter().map(|(p, _)| p.clone()).collect();
    let changed_refs: Vec<&Path> = changed_paths.iter().map(|p| p.as_path()).collect();

    let hook_only_compile = result.compile_queue.is_empty()
        && !result.asset_changed.is_empty()
        && crate::hooks::has_watched_hooks(config, &changed_refs);

    if !result.asset_changed.is_empty() {
        messages.push(CompilerMsg::AssetChange(result.asset_changed));
    }

    // Output changes are tracked separately:
    // - classified modified output files
    // - created/removed output files from raw events
    let mut output_changed = result.output_changed;
    output_changed.extend(
        created
            .iter()
            .filter(|p| matches!(categorize_path(p, config), FileCategory::Output))
            .cloned(),
    );
    output_changed.extend(
        removed
            .iter()
            .filter(|p| matches!(categorize_path(p, config), FileCategory::Output))
            .cloned(),
    );
    if !output_changed.is_empty() {
        let mut seen = FxHashSet::default();
        output_changed.retain(|p| seen.insert(p.clone()));
        messages.push(CompilerMsg::OutputChange(output_changed));
    }

    if !result.compile_queue.is_empty() || hook_only_compile {
        messages.push(CompilerMsg::Compile {
            queue: result.compile_queue,
            changed_paths,
        });
    }

    messages
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashMap;
    use tempfile::TempDir;

    use super::super::classifier::EventClassifier;
    use super::super::types::{ChangeKind, DebouncedEvents};
    use super::*;
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

    #[test]
    fn output_changes_are_routed_to_output_change_message() {
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
    fn asset_change_without_watched_hooks_does_not_enqueue_compile() {
        let (_tmp, mut config) = make_config();
        let root = config.get_root().to_path_buf();
        config.build.assets.normalize(&root);
        let asset = config.get_root().join("assets/styles/tailwind.css");
        let events = DebouncedEvents(vec![(asset, ChangeKind::Modified)]);

        let messages = events_to_messages(events, &config);

        assert!(
            messages
                .iter()
                .any(|msg| matches!(msg, CompilerMsg::AssetChange(_)))
        );
        assert!(
            !messages
                .iter()
                .any(|msg| matches!(msg, CompilerMsg::Compile { .. }))
        );
    }

    #[test]
    fn created_asset_enqueues_asset_change() {
        let (_tmp, mut config) = make_config();
        let root = config.get_root().to_path_buf();
        config.build.assets.normalize(&root);
        let asset = config.get_root().join("assets/styles/tailwind.css");
        let events = DebouncedEvents(vec![(asset.clone(), ChangeKind::Created)]);

        let messages = events_to_messages(events, &config);
        let asset_msg = messages.into_iter().find_map(|msg| match msg {
            CompilerMsg::AssetChange(paths) => Some(paths),
            _ => None,
        });

        let paths = asset_msg.expect("expected AssetChange message");
        assert_eq!(paths, vec![asset]);
    }

    #[test]
    fn removed_dep_without_known_dependents_triggers_full_rebuild() {
        crate::compiler::dependency::clear_graph();

        let (_tmp, mut config) = make_config();
        let root = config.get_root().to_path_buf();
        let deps_dir = root.join("templates");
        config.build.deps = vec![deps_dir.clone()];
        std::fs::create_dir_all(&deps_dir).unwrap();

        let removed_dep = deps_dir.join("base.typ");
        let mut raw = FxHashMap::default();
        raw.insert(removed_dep, ChangeKind::Removed);

        let events = EventClassifier::classify(raw, &config).expect("expected actionable event");
        let messages = events_to_messages(events, &config);

        assert!(
            messages
                .iter()
                .any(|msg| matches!(msg, CompilerMsg::FullRebuild))
        );
    }

    #[test]
    fn asset_change_with_watched_hooks_enqueues_hook_only_compile() {
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

        assert!(
            messages
                .iter()
                .any(|msg| matches!(msg, CompilerMsg::AssetChange(_)))
        );

        let compile = messages.into_iter().find_map(|msg| match msg {
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
}
