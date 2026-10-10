//! The files and directories one listing shows, rendered as a tree.

use std::collections::BTreeMap;
use std::path::Path;

/// What one entry of a tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Directory,
    File,
}

/// The entries one tree lists, keyed by name at each level.
#[derive(Default)]
pub(crate) struct Tree {
    kind: Option<EntryKind>,
    children: BTreeMap<String, Tree>,
}

impl Tree {
    /// Adds one entry: `path`'s last segment is `kind`, and every segment above it is the
    /// directory that holds it.
    pub(crate) fn insert(&mut self, path: &Path, kind: EntryKind) {
        let mut current = self;
        let mut segments = path.iter().peekable();
        while let Some(segment) = segments.next() {
            let name = segment.to_string_lossy();
            let child = current.children.entry(name.into_owned()).or_default();
            if segments.peek().is_some() && child.kind.is_none() {
                child.kind = Some(EntryKind::Directory);
            }
            current = child;
        }
        current.kind = Some(kind);
    }

    /// Every entry, one line each, joined by the glyphs that reach it; an empty tree
    /// renders nothing.
    pub(crate) fn render(&self) -> String {
        let mut output = String::new();
        self.render_children(&mut output, "");
        output.trim_end().to_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_renders_the_directories_that_hold_it() {
        let mut tree = Tree::default();
        tree.insert(Path::new("content/index.typ"), EntryKind::File);
        tree.insert(Path::new("site/page.typ"), EntryKind::File);
        tree.insert(Path::new("site.typ"), EntryKind::File);

        assert_eq!(
            tree.render(),
            "├── content/\n│   └── index.typ\n├── site/\n│   └── page.typ\n└── site.typ"
        );
    }

    #[test]
    fn declared_directory_renders_without_files() {
        let mut tree = Tree::default();
        tree.insert(Path::new("static/web"), EntryKind::Directory);
        tree.insert(Path::new("tola.toml"), EntryKind::File);

        assert_eq!(tree.render(), "├── static/\n│   └── web/\n└── tola.toml");
    }
}
