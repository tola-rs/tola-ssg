//! Rendering for the files and directories created by initialization.

use std::collections::BTreeMap;
use std::path::Path;

use crate::writes::FileWrites;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
}

pub(super) fn render(writes: &FileWrites) -> String {
    let mut tree = TreeNode::default();
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
    tree.render_children(&mut output, "");
    output.trim_end().to_owned()
}

#[derive(Default)]
struct TreeNode {
    kind: Option<EntryKind>,
    children: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    fn insert(&mut self, path: &Path, kind: EntryKind) {
        let mut current = self;
        let mut segments = path.iter().peekable();
        while let Some(segment) = segments.next() {
            let name = segment.to_string_lossy();
            let child = current.child_mut(&name);
            if segments.peek().is_some() && child.kind.is_none() {
                child.kind = Some(EntryKind::Directory);
            }
            current = child;
        }
        current.kind = Some(kind);
    }

    fn child_mut(&mut self, name: &str) -> &mut TreeNode {
        self.children.entry(name.to_owned()).or_default()
    }

    fn render_children(&self, output: &mut String, prefix: &str) {
        for (index, (name, child)) in self.children.iter().enumerate() {
            let last = index + 1 == self.children.len();
            output.push_str(prefix);
            output.push_str(if last { "└── " } else { "├── " });
            output.push_str(name);
            if child.kind == Some(EntryKind::Directory) || !child.children.is_empty() {
                output.push('/');
            }
            output.push('\n');
            if !child.children.is_empty() {
                let next = if last {
                    format!("{prefix}    ")
                } else {
                    format!("{prefix}│   ")
                };
                child.render_children(output, &next);
            }
        }
    }
}
