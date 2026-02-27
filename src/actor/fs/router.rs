use std::path::{Path, PathBuf};

use rustc_hash::FxHashSet;

use super::types::DebouncedEvents;
use crate::actor::messages::CompilerMsg;
use crate::config::SiteConfig;
use crate::reload::classify::classify_changes;

pub(super) fn log_events(events: &DebouncedEvents) {
    for (path, kind) in &events.0 {
        crate::debug!("watch"; "{}: {}", kind.label(), path.display());
    }
}

/// Convert DebouncedEvents to CompilerMsg(s)
pub(super) fn events_to_messages(events: DebouncedEvents, config: &SiteConfig) -> Vec<CompilerMsg> {
    use crate::reload::classify::{FileCategory, categorize_path};

    let (created, modified, removed) = events.split();

    // Classify modified files through the existing pipeline
    let result = classify_changes(&modified, config);

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
