//! Rendering for the files and directories created by initialization.

use crate::tree::{EntryKind, Tree};
use crate::writes::FileWrites;

use tola_build::filesystem::layout;

pub(super) fn render(writes: &FileWrites) -> String {
    let mut tree = Tree::default();
    for directory in writes.directory_paths() {
        tree.insert(writes.relative_path(directory), EntryKind::Directory);
    }
    let mirror = layout::builtin_packages_relative();
    let mut has_package_files = false;
    for file in writes.file_paths() {
        let relative = writes.relative_path(file);
        if relative.starts_with(&mirror) {
            has_package_files = true;
        } else {
            tree.insert(relative, EntryKind::File);
        }
    }
    if has_package_files {
        tree.insert(&mirror, EntryKind::Directory);
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
