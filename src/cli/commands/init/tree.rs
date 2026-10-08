//! Rendering for the files and directories created by initialization.

use std::path::Path;

use crate::tree::{EntryKind, Tree};
use crate::writes::FileWrites;

pub(super) fn render(writes: &FileWrites) -> String {
    let mut tree = Tree::default();
    for directory in writes.directory_paths() {
        tree.insert(writes.relative_path(directory), EntryKind::Directory);
    }
    let mut has_package_files = false;
    for file in writes.file_paths() {
        let relative = writes.relative_path(file);
        if relative.starts_with(crate::editor::GENERATED_PACKAGE_DIRECTORY) {
            has_package_files = true;
        } else {
            tree.insert(relative, EntryKind::File);
        }
    }
    if has_package_files {
        tree.insert(
            Path::new(crate::editor::GENERATED_PACKAGE_DIRECTORY),
            EntryKind::Directory,
        );
    }

    let name = writes
        .root()
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(".");
    let mut output = format!("{name}/\n");
    output.push_str(&tree.render());
    output.trim_end().to_owned()
}
