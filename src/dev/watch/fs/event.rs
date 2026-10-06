//! Event kinds and logical paths shared by observation, coverage, and debouncing.

use std::path::PathBuf;

use notify::EventKind;
use notify::event::{CreateKind, ModifyKind, RemoveKind};

pub(super) fn accepts_event(kind: EventKind) -> bool {
    match kind {
        EventKind::Any | EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Metadata(kind)) => accepts_metadata(kind),
        EventKind::Modify(_) => true,
        _ => false,
    }
}

fn accepts_metadata(kind: notify::event::MetadataKind) -> bool {
    !matches!(
        kind,
        notify::event::MetadataKind::AccessTime | notify::event::MetadataKind::WriteTime
    )
}

/// Whether an event reaches a subscription's whole scope whatever a path is called.
///
/// A created, removed, or renamed tree, and an untyped event, can stand for a subtree whose
/// membership moved, so those kinds keep the whole scope: a removed directory has no file name
/// and its subtree reports nothing. A file-level create or remove names one path that nothing
/// moves under, so the file's own name decides through each subscription's admission instead;
/// macOS reports an ordinary write to an existing file as a create, and admitting that by scope
/// would rebuild for every write under a watched tree whatever its name. A metadata change names
/// one path and moves nothing either, and macOS reports inode metadata beside an ordinary write.
/// The catch-all modify kinds stay narrowable for the same reason: the Windows backend reports
/// `FILE_ACTION_MODIFIED` as `Modify(Any)`. A create or remove that does not prove it is a file
/// is treated as a tree.
pub(super) fn is_structural(kind: EventKind) -> bool {
    match kind {
        EventKind::Create(CreateKind::File) | EventKind::Remove(RemoveKind::File) => false,
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Any | EventKind::Modify(ModifyKind::Name(_)) => true,
        _ => false,
    }
}

pub(super) fn accepted_root_invalidation_kind(kind: EventKind) -> bool {
    if let EventKind::Modify(ModifyKind::Metadata(kind)) = kind {
        return accepts_metadata(kind);
    }
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Any
    )
}

pub(super) fn normalized_event_paths(event: &notify::Event) -> impl Iterator<Item = PathBuf> + '_ {
    event
        .paths
        .iter()
        .map(|path| tola_build::filesystem::lexical_path_identity(path))
}
