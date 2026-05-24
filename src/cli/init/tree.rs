//! Init site tree.

use anyhow::{Context, Result};
use std::{fs, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitTreeEntryKind {
    Dir,
    File,
}

#[derive(Debug, Clone, Copy)]
struct InitTreeEntry {
    path: &'static str,
    kind: InitTreeEntryKind,
}

impl InitTreeEntry {
    const fn dir(path: &'static str) -> Self {
        Self {
            path,
            kind: InitTreeEntryKind::Dir,
        }
    }

    const fn file(path: &'static str) -> Self {
        Self {
            path,
            kind: InitTreeEntryKind::File,
        }
    }
}

const BASE_TREE_HEAD: &[InitTreeEntry] = &[
    InitTreeEntry::file("tola.toml"),
    InitTreeEntry::file(".gitignore"),
    InitTreeEntry::file(".ignore"),
    InitTreeEntry::dir("content"),
    InitTreeEntry::dir("assets/images"),
    InitTreeEntry::dir("assets/fonts"),
    InitTreeEntry::dir("assets/scripts"),
    InitTreeEntry::dir("assets/styles"),
];

const TOLA_LIB_TREE: &[InitTreeEntry] = &[
    InitTreeEntry::dir("tola"),
    InitTreeEntry::file("tola/lib.typ"),
];

const BASE_TREE_TAIL: &[InitTreeEntry] =
    &[InitTreeEntry::dir("templates"), InitTreeEntry::dir("utils")];

fn entries(include_tola_lib: bool) -> impl Iterator<Item = &'static InitTreeEntry> {
    BASE_TREE_HEAD
        .iter()
        .chain(
            include_tola_lib
                .then_some(TOLA_LIB_TREE)
                .into_iter()
                .flatten(),
        )
        .chain(BASE_TREE_TAIL)
}

fn dirs(include_tola_lib: bool) -> impl Iterator<Item = &'static str> {
    entries(include_tola_lib)
        .filter(|entry| entry.kind == InitTreeEntryKind::Dir)
        .map(|entry| entry.path)
}

pub fn create_dirs(root: &Path, include_tola_lib: bool) -> Result<()> {
    if !root.exists() {
        fs::create_dir_all(root)
            .with_context(|| format!("Failed to create root directory '{}'", root.display()))?;
    }

    for dir in dirs(include_tola_lib) {
        let path = root.join(dir);
        fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create directory '{}'", path.display()))?;
    }

    Ok(())
}

pub fn render_tree(root: &Path, include_tola_lib: bool) -> String {
    let mut tree = TreeNode::default();
    for entry in entries(include_tola_lib) {
        tree.insert(entry.path, entry.kind);
    }

    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(".");

    let mut out = String::new();
    out.push_str(name);
    out.push_str("/\n");
    tree.render_children(&mut out, "");
    out.trim_end().to_string()
}

#[derive(Default)]
struct TreeNode {
    kind: Option<InitTreeEntryKind>,
    children: Vec<(&'static str, TreeNode)>,
}

impl TreeNode {
    fn insert(&mut self, path: &'static str, kind: InitTreeEntryKind) {
        let mut current = self;
        let mut segments = path.split('/').peekable();
        while let Some(segment) = segments.next() {
            let child = current.child_mut(segment);
            if segments.peek().is_some() && child.kind.is_none() {
                child.kind = Some(InitTreeEntryKind::Dir);
            }
            current = child;
        }
        current.kind = Some(kind);
    }

    fn child_mut(&mut self, name: &'static str) -> &mut TreeNode {
        if let Some(idx) = self
            .children
            .iter()
            .position(|(child_name, _)| *child_name == name)
        {
            &mut self.children[idx].1
        } else {
            self.children.push((name, TreeNode::default()));
            &mut self.children.last_mut().unwrap().1
        }
    }

    fn render_children(&self, out: &mut String, prefix: &str) {
        let len = self.children.len();
        for (idx, (name, child)) in self.children.iter().enumerate() {
            let last = idx + 1 == len;
            out.push_str(prefix);
            out.push_str(if last { "└── " } else { "├── " });
            out.push_str(name);
            if child.kind == Some(InitTreeEntryKind::Dir) || !child.children.is_empty() {
                out.push('/');
            }
            out.push('\n');

            if !child.children.is_empty() {
                let next_prefix = if last {
                    format!("{prefix}    ")
                } else {
                    format!("{prefix}│   ")
                };
                child.render_children(out, &next_prefix);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn renders_init_tree_entries() {
        let tree = render_tree(Path::new("/tmp/my-site"), true);

        assert!(tree.contains("my-site/"));
        assert!(tree.contains("├── assets/"));
        assert!(tree.contains("├── tola/"));
        assert!(tree.contains("│   └── lib.typ"));
        assert!(tree.contains("├── templates/"));
        assert!(tree.contains("│   ├── fonts/"));
        assert!(tree.contains("└── utils/"));
    }

    #[test]
    fn creates_init_tree_dirs() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("my-site");

        create_dirs(&root, true).unwrap();

        assert!(root.join("content").is_dir());
        assert!(root.join("assets/images").is_dir());
        assert!(root.join("tola").is_dir());
        assert!(root.join("templates").is_dir());
        assert!(root.join("utils").is_dir());
    }

    #[test]
    fn creates_dirs_in_existing_root() {
        let temp = TempDir::new().unwrap();

        create_dirs(temp.path(), true).unwrap();

        assert!(temp.path().join("content").is_dir());
    }
}
