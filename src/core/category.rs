//! File category definitions.

use std::path::Path;

/// Kind of content file, determines compilation strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentKind {
    /// Typst file (.typ) - compile with typst-batch
    Typst,
    /// Markdown file (.md) - compile with pulldown-cmark
    Markdown,
}

impl ContentKind {
    /// Detect content kind from file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "typ" | "typst" => Some(Self::Typst),
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    /// Detect content kind from file path.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Self::from_extension)
    }

    /// File extensions for this content kind.
    #[allow(dead_code)]
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Typst => &["typ", "typst"],
            Self::Markdown => &["md", "markdown"],
        }
    }

    /// Display name for this content kind.
    pub fn name(self) -> &'static str {
        match self {
            Self::Typst => "typst",
            Self::Markdown => "markdown",
        }
    }

    /// Check if a path is a content file.
    #[inline]
    pub fn is_content_file(path: &Path) -> bool {
        Self::from_path(path).is_some()
    }

    /// Partition files by content kind. Returns `(typst_files, markdown_files)`.
    pub fn partition_by_kind<P: AsRef<Path>>(files: &[P]) -> (Vec<&P>, Vec<&P>) {
        files
            .iter()
            .partition(|p| Self::from_path(p.as_ref()) == Some(Self::Typst))
    }
}

/// Category of a file, determines rebuild strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    /// Content file - compile based on kind
    Content(ContentKind),
    /// Asset file - copy to output
    Asset,
    /// Site config (tola.toml) - full rebuild
    Config,
    /// Dependency (templates, utils) - rebuild dependents
    Deps,
    /// Output file - trigger hot reload (from hooks)
    Output,
    /// Outside watched dirs - ignored
    Unknown,
}

impl FileCategory {
    pub fn name(self) -> &'static str {
        match self {
            Self::Content(kind) => kind.name(),
            Self::Asset => "asset",
            Self::Config => "config",
            Self::Deps => "deps",
            Self::Output => "output",
            Self::Unknown => "unknown",
        }
    }

    /// Returns true if this is a content type that needs compilation.
    #[allow(dead_code)]
    pub fn is_content(self) -> bool {
        matches!(self, Self::Content(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_from_extension() {
        assert_eq!(ContentKind::from_extension("typ"), Some(ContentKind::Typst));
        assert_eq!(
            ContentKind::from_extension("md"),
            Some(ContentKind::Markdown)
        );
        assert_eq!(ContentKind::from_extension("html"), None);
    }

    #[test]
    fn test_from_path() {
        assert_eq!(
            ContentKind::from_path(&PathBuf::from("post.typ")),
            Some(ContentKind::Typst)
        );
        assert_eq!(
            ContentKind::from_path(&PathBuf::from("readme.md")),
            Some(ContentKind::Markdown)
        );
    }

    #[test]
    fn test_is_content_file() {
        assert!(ContentKind::is_content_file(&PathBuf::from("post.typ")));
        assert!(ContentKind::is_content_file(&PathBuf::from("readme.md")));
        assert!(ContentKind::is_content_file(&PathBuf::from("doc.markdown")));
        assert!(ContentKind::is_content_file(&PathBuf::from("file.typst")));
        assert!(!ContentKind::is_content_file(&PathBuf::from("image.png")));
        assert!(!ContentKind::is_content_file(&PathBuf::from("style.css")));
        assert!(!ContentKind::is_content_file(&PathBuf::from("noext")));
    }

    #[test]
    fn test_partition_by_kind() {
        let files = vec![
            PathBuf::from("a.typ"),
            PathBuf::from("b.md"),
            PathBuf::from("c.typ"),
            PathBuf::from("d.markdown"),
        ];
        let (typst, markdown) = ContentKind::partition_by_kind(&files);

        assert_eq!(typst.len(), 2);
        assert_eq!(markdown.len(), 2);
        assert!(typst.iter().all(|p| p.extension().unwrap() == "typ"));
        assert!(markdown.iter().all(|p| {
            let ext = p.extension().unwrap().to_str().unwrap();
            ext == "md" || ext == "markdown"
        }));
    }

    #[test]
    fn test_partition_by_kind_empty() {
        let files: Vec<PathBuf> = vec![];
        let (typst, markdown) = ContentKind::partition_by_kind(&files);
        assert!(typst.is_empty());
        assert!(markdown.is_empty());
    }

    #[test]
    fn test_partition_by_kind_only_typst() {
        let files = vec![PathBuf::from("a.typ"), PathBuf::from("b.typst")];
        let (typst, markdown) = ContentKind::partition_by_kind(&files);
        assert_eq!(typst.len(), 2);
        assert!(markdown.is_empty());
    }

    #[test]
    fn test_partition_by_kind_only_markdown() {
        let files = vec![PathBuf::from("a.md"), PathBuf::from("b.markdown")];
        let (typst, markdown) = ContentKind::partition_by_kind(&files);
        assert!(typst.is_empty());
        assert_eq!(markdown.len(), 2);
    }
}
