use super::ignore::IgnoreMatcher;
use anyhow::{Context, Result, anyhow};
use gix::{
    ThreadSafeRepository,
    bstr::BString,
    index::{
        State,
        entry::{Flags, Mode, Stat},
        fs::Metadata,
    },
    objs::{Tree, tree},
};
use std::{fs, path::Path};

/// Builder for constructing git trees from the filesystem
pub struct TreeBuilder<'a> {
    repo: &'a ThreadSafeRepository,
    matcher: IgnoreMatcher,
}

impl<'a> TreeBuilder<'a> {
    pub fn new(repo: &'a ThreadSafeRepository, gitignore: &'a [u8]) -> Self {
        Self {
            repo,
            matcher: IgnoreMatcher::new(gitignore),
        }
    }

    /// Build a git tree from a directory
    ///
    /// Recursively traverses the directory, creating blobs for files and trees for subdirectories.
    /// Respects .gitignore rules.
    pub fn build_from_dir(&self, dir: &Path, index: &mut State) -> Result<Tree> {
        let repo_local = self.repo.to_thread_local();
        let repo_root = self.repo.path().parent().context("Invalid repo path")?;

        let mut entries = Vec::new();

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let filename = self.get_filename(&entry)?;
            let rel_path = path.strip_prefix(repo_root)?.to_string_lossy();
            let is_dir = path.is_dir();

            // Skip ignored and .git directory
            if self.should_ignore(&rel_path, &filename, is_dir) {
                continue;
            }

            if is_dir {
                // Recursively build tree for subdirectory
                let sub_tree = self.build_from_dir(&path, index)?;
                let tree_id = repo_local.write_object(&sub_tree)?.detach();
                entries.push(self.create_tree_entry(filename, tree_id));
            } else if path.is_file() {
                // Create blob for file and add to index
                let blob_id = self.write_blob(&repo_local, &path)?;
                self.add_to_index(index, &path, blob_id, &filename)?;
                entries.push(self.create_blob_entry(filename, blob_id));
            }
        }

        // Sort entries according to git tree ordering
        sort_tree_entries(&mut entries);

        Ok(Tree { entries })
    }

    /// Get filename as BString
    fn get_filename(&self, entry: &fs::DirEntry) -> Result<BString> {
        entry
            .file_name()
            .into_string()
            .map(Into::into)
            .map_err(|_| anyhow!("Invalid UTF-8 in filename"))
    }

    /// Check if path should be ignored
    fn should_ignore(&self, rel_path: &str, filename: &BString, is_dir: bool) -> bool {
        filename == ".git" || self.matcher.matches(rel_path, is_dir)
    }

    /// Write file contents as blob
    fn write_blob(&self, repo: &gix::Repository, path: &Path) -> Result<gix::ObjectId> {
        let contents = fs::read(path)?;
        Ok(repo.write_blob(contents)?.into())
    }

    /// Add file to index
    fn add_to_index(
        &self,
        index: &mut State,
        path: &Path,
        blob_id: gix::ObjectId,
        filename: &BString,
    ) -> Result<()> {
        let stat = Stat::from_fs(&Metadata::from_path_no_follow(path)?)?;
        index.dangerously_push_entry(stat, blob_id, Flags::empty(), Mode::FILE, filename.as_ref());
        Ok(())
    }

    /// Create a tree entry for a subdirectory
    fn create_tree_entry(&self, filename: BString, oid: gix::ObjectId) -> tree::Entry {
        tree::Entry {
            mode: tree::EntryKind::Tree.into(),
            oid,
            filename,
        }
    }

    /// Create a tree entry for a file
    fn create_blob_entry(&self, filename: BString, oid: gix::ObjectId) -> tree::Entry {
        tree::Entry {
            mode: tree::EntryKind::Blob.into(),
            oid,
            filename,
        }
    }
}

/// Sort entries according to git tree ordering (directories get trailing slash for comparison)
///
/// Git sorts tree entries by name, but treats directories as if they end with a slash.
/// This ensures that "foo" (file) comes before "foo-bar" (file), but "foo-bar" comes before "foo" (directory).
fn sort_tree_entries(entries: &mut [tree::Entry]) {
    let tree_mode: tree::EntryMode = tree::EntryKind::Tree.into();
    entries.sort_by(|a, b| {
        let sort_key = |e: &tree::Entry| {
            let mut key = e.filename.as_slice().to_vec();
            if e.mode == tree_mode {
                key.push(b'/');
            }
            key
        };
        sort_key(a).cmp(&sort_key(b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix::objs::tree::Entry;
    use gix::objs::tree::EntryKind;
    use std::fs::File;
    use tempfile::TempDir;

    fn with_temp_repo<F>(f: F)
    where
        F: FnOnce(&Path, &ThreadSafeRepository),
    {
        let temp_dir = TempDir::new().unwrap();
        let repo = gix::init(temp_dir.path()).unwrap().into_sync();
        f(temp_dir.path(), &repo);
        // TempDir automatically cleans up on drop
    }

    #[test]
    fn test_sort_tree_entries() {
        let mut entries = vec![
            Entry { mode: EntryKind::Blob.into(), filename: "foo.rs".into(), oid: gix::ObjectId::null(gix::hash::Kind::Sha1) },
            Entry { mode: EntryKind::Tree.into(), filename: "foo".into(), oid: gix::ObjectId::null(gix::hash::Kind::Sha1) },
            Entry { mode: EntryKind::Blob.into(), filename: "foo-bar".into(), oid: gix::ObjectId::null(gix::hash::Kind::Sha1) },
        ];

        // Git sort order:
        // "foo-bar" (45) < "foo.rs" (46) < "foo/" (47)

        sort_tree_entries(&mut entries);

        assert_eq!(entries[0].filename, "foo-bar");
        assert_eq!(entries[1].filename, "foo.rs");
        assert_eq!(entries[2].filename, "foo");
    }

    #[test]
    fn test_tree_builder_respects_gitignore() {
        with_temp_repo(|dir, repo| {
            // Create files
            File::create(dir.join("file.txt")).unwrap();
            File::create(dir.join("ignore.me")).unwrap();
            fs::create_dir(dir.join("ignored_dir")).unwrap();
            File::create(dir.join("ignored_dir/file")).unwrap();

            // Create .gitignore content (not file, passed to builder)
            let gitignore = b"*.me\nignored_dir/";

            let mut index = State::new(repo.to_thread_local().object_hash());
            let builder = TreeBuilder::new(repo, gitignore);
            let tree = builder.build_from_dir(dir, &mut index).unwrap();

            // Check tree entries
            assert_eq!(tree.entries.len(), 1);
            assert_eq!(tree.entries[0].filename, "file.txt");
        });
    }

    #[test]
    fn test_tree_builder_nested() {
        with_temp_repo(|dir, repo| {
            // Structure:
            // /main.rs
            // /src/
            //   /lib.rs

            File::create(dir.join("main.rs")).unwrap();
            fs::create_dir(dir.join("src")).unwrap();
            File::create(dir.join("src/lib.rs")).unwrap();

            let mut index = State::new(repo.to_thread_local().object_hash());
            let builder = TreeBuilder::new(repo, &[]);
            let tree = builder.build_from_dir(dir, &mut index).unwrap();

            assert_eq!(tree.entries.len(), 2);
            // Sorted: main.rs, src/ (because 'm' < 's')
            assert_eq!(tree.entries[0].filename, "main.rs");
            assert_eq!(tree.entries[1].filename, "src");
            assert_eq!(tree.entries[1].mode, EntryKind::Tree.into());
        });
    }
}
