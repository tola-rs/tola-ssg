//! Parsed sources reused while each source's revalidated bytes stay unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_typst::ContentDigest;
use tola_typst::typst::syntax::{FileId, Source};
use tola_typst_syntax::names::SourceNames;

/// Parsed names from the bytes a source walk or import closure last read. Reads revalidate the
/// bytes; reuse never substitutes a timestamp for the compiler's content identity. A completed
/// pass drops sources neither the walk nor its import closure reached.
#[derive(Default)]
pub(crate) struct DiskSources {
    pass: u64,
    parsed: HashMap<FileId, ParsedSource>,
    /// Committed with a completed pass; a cancelled read never publishes a partial alias set.
    aliases: HashMap<FileId, PathBuf>,
}

struct ParsedSource {
    /// The pass that last read the file, so a file the walk stopped seeing is dropped.
    pass: u64,
    /// The digest of the bytes this parse came from.
    digest: ContentDigest,
    names: Arc<SourceNames>,
    alias: Option<PathBuf>,
}

impl DiskSources {
    /// Start one pass over the site, which a completed walk closes with [`Self::retain_pass`].
    ///
    /// A pass aborted between reads — a cancellation inside the walk — stays open: what it parsed
    /// is kept, and the next completed pass re-reads each file's bytes and drops what it no longer
    /// sees.
    pub(crate) fn begin(&mut self) {
        self.pass += 1;
    }

    /// Read one site file and return its parsed names, parsing again when its bytes changed.
    pub(crate) fn read_names(&mut self, path: &Path, id: FileId) -> Option<Arc<SourceNames>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                tracing::debug!(
                    path = %path.display(),
                    %error,
                    "a site file the walk reached could not be read"
                );
                return None;
            }
        };
        let names = self.names_from_text(id, text);
        self.parsed.get_mut(&id).expect("the file was parsed").alias = None;
        Some(names)
    }

    /// Reuses the parse of these revalidated bytes, including hidden imports and package sources.
    pub(crate) fn names_from_text(&mut self, id: FileId, text: String) -> Arc<SourceNames> {
        let digest = ContentDigest::of(text.as_bytes());
        if let Some(parsed) = self.parsed.get_mut(&id)
            && parsed.digest == digest
        {
            parsed.pass = self.pass;
            return Arc::clone(&parsed.names);
        }
        let names = Arc::new(SourceNames::new(Source::new(id, text)));
        self.parsed.insert(
            id,
            ParsedSource {
                pass: self.pass,
                digest,
                names: Arc::clone(&names),
                alias: None,
            },
        );
        names
    }

    pub(super) fn set_alias(&mut self, id: FileId, path: Option<PathBuf>) {
        self.parsed
            .get_mut(&id)
            .expect("an alias belongs to a parsed source")
            .alias = path;
    }

    pub(super) fn source_aliases(&self, file: FileId, root: &Path) -> Vec<FileId> {
        if self.aliases.is_empty() {
            return vec![file];
        }
        let path = self
            .aliases
            .get(&file)
            .cloned()
            .unwrap_or_else(|| root.join(file.vpath().get_without_slash()));
        let mut files = vec![file];
        if let Some(physical) = tola_typst::file_id_from_path(&path, root) {
            files.push(physical);
        }
        files.extend(
            self.aliases
                .iter()
                .filter_map(|(alias, target)| (target == &path).then_some(*alias)),
        );
        files.sort_by(super::by_source_path);
        files.dedup();
        files
    }

    /// Close the pass [`Self::begin`] opened: drop every file this pass did not read.
    pub(crate) fn retain_pass(&mut self) {
        let pass = self.pass;
        self.parsed.retain(|_, parsed| parsed.pass == pass);
        self.aliases = self
            .parsed
            .iter()
            .filter_map(|(id, parsed)| parsed.alias.as_ref().map(|path| (*id, path.clone())))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfinished_reads_preserve_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let file = |name: &str| tola_typst::file_id_from_path(&root.join(name), &root).unwrap();
        let alias = file("alias.typ");
        let first = file("first.typ");
        let second = file("second.typ");
        let mut disk = DiskSources::default();
        disk.begin();
        disk.names_from_text(alias, "#let value = 1".into());
        disk.set_alias(alias, Some(root.join("first.typ")));
        disk.retain_pass();
        assert!(disk.source_aliases(first, &root).contains(&alias));
        disk.begin();
        disk.names_from_text(alias, "#let value = 2".into());
        disk.set_alias(alias, Some(root.join("second.typ")));
        assert!(disk.source_aliases(first, &root).contains(&alias));
        assert!(!disk.source_aliases(second, &root).contains(&alias));
        disk.begin();
        disk.names_from_text(alias, "#let value = 2".into());
        disk.set_alias(alias, Some(root.join("second.typ")));
        disk.retain_pass();
        assert!(!disk.source_aliases(first, &root).contains(&alias));
        assert!(disk.source_aliases(second, &root).contains(&alias));
        disk.begin();
        disk.retain_pass();
        assert!(!disk.source_aliases(second, &root).contains(&alias));
    }
}
