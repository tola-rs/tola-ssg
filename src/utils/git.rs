//! Git operations for the static site generator.
//!
//! Handles repository initialization, commits, and remote pushing.

use crate::{config::SiteConfig, exec, init::init_ignored_files, log};
use anyhow::{Context, Result, anyhow, bail};
use gix::{
    Repository, ThreadSafeRepository,
    bstr::{BString, ByteSlice},
    commit::NO_PARENT_IDS,
    glob::wildmatch,
    index::{
        State,
        entry::{Flags, Mode, Stat},
        fs::Metadata,
    },
    objs::{Tree, tree},
    remote::Direction,
};
use std::{fs, path::Path};

// ============================================================================
// Repository Operations
// ============================================================================

/// Create a new git repository at the given path
pub fn create_repo(root: &Path) -> Result<ThreadSafeRepository> {
    let repo = gix::init(root)?;
    init_ignored_files(root, &[Path::new(".DS_Store")])?;
    Ok(repo.into_sync())
}

/// Open an existing git repository
pub fn open_repo(root: &Path) -> Result<ThreadSafeRepository> {
    let repo = gix::open(root)?;
    Ok(repo.into_sync())
}

/// Commit all changes in the repository
pub fn commit_all(repo: &ThreadSafeRepository, message: &str) -> Result<()> {
    if message.trim().is_empty() {
        bail!("Commit message cannot be empty");
    }

    let repo_local = repo.to_thread_local();
    let root = get_repo_root(&repo_local)?;
    let gitignore_patterns = read_gitignore(root)?;

    // Build index and tree from working directory
    let mut index = State::new(repo_local.object_hash());
    let tree = TreeBuilder::new(repo, &gitignore_patterns).build_from_dir(root, &mut index)?;
    index.sort_entries();

    // Write index file
    let mut index_file = gix::index::File::from_state(index, repo_local.index_path());
    index_file.write(gix::index::write::Options::default())?;

    // Create commit
    let tree_id = repo_local.write_object(&tree)?;
    let parent_ids = get_parent_commit_ids(repo)?;
    let commit_id = repo_local.commit("HEAD", message, tree_id, parent_ids)?;

    log!("git"; "commit {commit_id}");
    Ok(())
}

/// Push commits to remote repository
pub fn push(repo: &ThreadSafeRepository, config: &'static SiteConfig) -> Result<()> {
    let github = &config.deploy.github;
    log!("git"; "pushing to {}", github.url);

    let repo_local = repo.to_thread_local();
    let root = get_repo_root(&repo_local)?;

    // Setup remote
    let remote_url = build_authenticated_url(&github.url, github.token_path.as_ref())?;
    configure_origin_remote(root, &repo_local, &remote_url)?;

    // Push to remote
    push_to_remote(root, &github.branch, config.deploy.force)?;

    // Verify remote configuration
    if !config.deploy.force && !Remote::origin_matches(&repo_local, &remote_url)? {
        bail!(
            "Remote origin URL in `{root:?}` doesn't match [deploy.git] config. \
             Enable [deploy.force] or fix manually."
        );
    }

    Ok(())
}

// ============================================================================
// Remote Management
// ============================================================================

struct Remote;

impl Remote {
    /// Check if origin remote exists with matching URL
    fn origin_matches(repo: &Repository, expected_url: &str) -> Result<bool> {
        if let Ok(remote) = repo.find_remote("origin") {
            if let Some(url) = remote
                .url(Direction::Push)
                .or_else(|| remote.url(Direction::Fetch))
            {
                return Ok(url.to_bstring().to_string() == expected_url);
            }
        }
        Ok(false)
    }

    /// Check if origin remote exists
    fn origin_exists(repo: &Repository) -> Result<bool> {
        Ok(repo.find_remote("origin").is_ok())
    }
}

/// Configure origin remote (add or update URL)
fn configure_origin_remote(root: &Path, repo: &Repository, url: &str) -> Result<()> {
    let action = if Remote::origin_exists(repo)? {
        "set-url"
    } else {
        "add"
    };
    exec!(root; ["git"]; "remote", action, "origin", url)?;
    Ok(())
}

/// Push to remote with optional force flag
fn push_to_remote(root: &Path, branch: &str, force: bool) -> Result<()> {
    if force {
        exec!(root; ["git"]; "push", "--set-upstream", "origin", branch, "-f")?;
    } else {
        exec!(root; ["git"]; "push", "--set-upstream", "origin", branch)?;
    }
    Ok(())
}

/// Build authenticated HTTPS URL with optional token
fn build_authenticated_url(url: &str, token_path: Option<&std::path::PathBuf>) -> Result<String> {
    let base_url = url
        .strip_prefix("https://")
        .context("Remote URL must start with https://")?;

    let token = token_path
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    match token {
        Some(token) => Ok(format!("https://{token}@{base_url}")),
        None => Ok(format!("https://{base_url}")),
    }
}

// ============================================================================
// Tree Building
// ============================================================================

/// Matches paths against .gitignore patterns.
///
/// This struct handles the complexity of gitignore rules, including:
/// - Pattern negation (!)
/// - Directory-only matches (ending with /)
/// - Absolute paths (starting with /)
/// - Basename vs path-relative matching
struct IgnoreMatcher {
    // Store (pattern_text, mode_bits)
    patterns: Vec<(BString, u32)>,
}

// Constants for gix::ignore::search::pattern::Mode (which is private)
// See: https://github.com/Byron/gitoxide/blob/main/gix-ignore/src/search/pattern.rs
const MODE_NO_SUB_DIR: u32 = 1 << 0;      // Pattern has no internal slash (matches basename unless absolute)
// const MODE_ENDS_WITH: u32 = 1 << 1;    // Pattern ends with something (not used here directly)
const MODE_MUST_MATCH_DIR: u32 = 1 << 2;  // Pattern ends with slash (must match directory)
const MODE_NEGATIVE: u32 = 1 << 3;        // Pattern starts with ! (negation)
const MODE_ABSOLUTE: u32 = 1 << 4;        // Pattern starts with / (rooted at gitignore location)

impl IgnoreMatcher {
    /// Parse gitignore bytes into patterns
    fn new(gitignore: &[u8]) -> Self {
        let patterns: Vec<(BString, u32)> = gix::ignore::parse(gitignore)
            .map(|(pattern, _, _)| (pattern.text, pattern.mode.bits()))
            .collect();
        Self { patterns }
    }

    /// Check if a path matches any ignore pattern
    ///
    /// Implements git's ignore logic:
    /// - Iterates patterns in order (last match wins)
    /// - Handles negation (!)
    /// - Handles directory-only patterns (ending in /)
    /// - Handles basename vs path-relative matching
    fn matches(&self, path: &str, is_dir: bool) -> bool {
        let mut is_ignored = false;
        for (text, mode) in &self.patterns {
            // If pattern must match a directory but path is not a directory, skip
            // e.g. "build/" should not match a file named "build"
            if (mode & MODE_MUST_MATCH_DIR != 0) && !is_dir {
                continue;
            }

            let mut match_path = path;
            let text_bytes = text.as_bstr();

            let is_absolute = mode & MODE_ABSOLUTE != 0;
            let has_internal_slash = mode & MODE_NO_SUB_DIR == 0;

            // If pattern is not absolute and has no internal slash, it matches against the basename.
            // Example: "*.log" matches "src/error.log" (basename "error.log").
            // Example: "/root.log" matches "root.log" but NOT "src/root.log".
            // Example: "target/debug" has slash, so it matches path-relatively.
            if !has_internal_slash && !is_absolute {
                if let Some(idx) = path.rfind('/') {
                    match_path = &path[idx + 1..];
                }
            }

            let is_match = wildmatch(
                text_bytes,
                match_path.into(),
                wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
            );

            if is_match {
                // If it's a negative match (starts with !), we un-ignore it.
                // Otherwise, we ignore it.
                if mode & MODE_NEGATIVE != 0 {
                    is_ignored = false;
                } else {
                    is_ignored = true;
                }
            }
        }
        is_ignored
    }
}

/// Builder for constructing git trees from the filesystem
struct TreeBuilder<'a> {
    repo: &'a ThreadSafeRepository,
    matcher: IgnoreMatcher,
}

impl<'a> TreeBuilder<'a> {
    fn new(repo: &'a ThreadSafeRepository, gitignore: &'a [u8]) -> Self {
        Self {
            repo,
            matcher: IgnoreMatcher::new(gitignore),
        }
    }

    /// Build a git tree from a directory
    ///
    /// Recursively traverses the directory, creating blobs for files and trees for subdirectories.
    /// Respects .gitignore rules.
    fn build_from_dir(&self, dir: &Path, index: &mut State) -> Result<Tree> {
        let repo_local = self.repo.to_thread_local();
        let repo_root = self.repo.path().parent().context("Invalid repo path")?;

        let mut entries = Vec::new();

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let filename = self.get_filename(&entry)?;
            let relative_path = path.strip_prefix(repo_root)?.to_string_lossy();
            let is_dir = path.is_dir();

            // Skip ignored and .git directory
            if self.should_ignore(&relative_path, &filename, is_dir) {
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
    fn should_ignore(&self, relative_path: &str, filename: &BString, is_dir: bool) -> bool {
        filename == ".git" || self.matcher.matches(relative_path, is_dir)
    }

    /// Write file contents as blob
    fn write_blob(&self, repo: &Repository, path: &Path) -> Result<gix::ObjectId> {
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
/// Wait, actually:
/// "foo" (file) < "foo-bar" (file)
/// "foo-bar" (file) < "foo/" (directory)
/// So "foo" < "foo-bar" < "foo/"
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

// ============================================================================
// Helper Functions
// ============================================================================

/// Get repository root path
fn get_repo_root(repo: &Repository) -> Result<&Path> {
    repo.path()
        .parent()
        .ok_or_else(|| anyhow!("Invalid repository path"))
}

/// Read .gitignore file if it exists
fn read_gitignore(root: &Path) -> Result<Vec<u8>> {
    let path = root.join(".gitignore");
    if path.exists() {
        Ok(fs::read(path)?)
    } else {
        Ok(Vec::new())
    }
}

/// Get parent commit IDs (empty for initial commit)
fn get_parent_commit_ids(repo: &ThreadSafeRepository) -> Result<Vec<gix::ObjectId>> {
    let repo_local = repo.to_thread_local();

    let parent_ids = repo_local
        .find_reference("refs/heads/main")
        .ok()
        .map(|refs| vec![refs.target().id().to_owned()])
        .unwrap_or_else(|| NO_PARENT_IDS.to_vec());

    Ok(parent_ids)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn with_temp_dir<F>(f: F)
    where
        F: FnOnce(&Path),
    {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("tola_test_{}", unique));
        fs::create_dir_all(&temp_dir).unwrap();

        f(&temp_dir);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_build_authenticated_url_no_token() {
        let url = "https://github.com/user/repo.git";
        let result = build_authenticated_url(url, None).unwrap();
        assert_eq!(result, "https://github.com/user/repo.git");
    }

    #[test]
    fn test_build_authenticated_url_with_token() {
        with_temp_dir(|dir| {
            let token_path = dir.join("token");
            let mut file = File::create(&token_path).unwrap();
            write!(file, "ghp_secret123").unwrap();

            let url = "https://github.com/user/repo.git";
            let result = build_authenticated_url(url, Some(&token_path)).unwrap();
            assert_eq!(result, "https://ghp_secret123@github.com/user/repo.git");
        });
    }

    #[test]
    fn test_build_authenticated_url_invalid_scheme() {
        let url = "http://github.com/user/repo.git";
        let result = build_authenticated_url(url, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_gix_parse_behavior() {
        let gitignore = b"/root_only\nsub/dir\n*.log\ntemp/";
        for (pattern, _, _) in gix::ignore::parse(gitignore) {
            println!("Pattern: {:?}, Mode: {:?} (bits: {:b})", pattern.text, pattern.mode, pattern.mode.bits());
        }
    }

    #[test]
    fn test_ignore_matcher() {
        // Note: We use "target/**" to match nested files with simple wildmatch
        let gitignore = b"target/**\n*.log\n.DS_Store\n!important.log\nbuild/\n/root_only";
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("target/debug/tola", false));
        assert!(matcher.matches("error.log", false));
        assert!(matcher.matches(".DS_Store", false));
        assert!(!matcher.matches("important.log", false));

        assert!(matcher.matches("build", true)); // Should match directory
        assert!(!matcher.matches("build", false)); // Should NOT match file named build

        // Absolute path test
        assert!(matcher.matches("root_only", false)); // Matches at root
        assert!(!matcher.matches("src/root_only", false)); // Should NOT match in subdir

        assert!(!matcher.matches("src/main.rs", false));
        assert!(!matcher.matches("README.md", false));
    }

    #[test]
    fn test_ignore_matcher_edge_cases() {
        // Note: Indentation in string literal is part of the content!
        // We should use a string without leading spaces for testing parsing.
        let gitignore = b"# This is a comment
*.tmp
!important.tmp
/TODO
docs/*.md";
        let matcher = IgnoreMatcher::new(gitignore);

        // Debug patterns
        // for (text, mode) in &matcher.patterns {
        //     println!("Pattern: {:?}, Mode: {:b}", text, mode);
        // }

        // Comments
        assert!(!matcher.matches("# This is a comment", false));

        // Simple wildcard
        assert!(matcher.matches("file.tmp", false));
        assert!(matcher.matches("dir/file.tmp", false));

        // Negation
        assert!(!matcher.matches("important.tmp", false));

        // Anchored
        assert!(matcher.matches("TODO", false));
        assert!(!matcher.matches("src/TODO", false));

        // Nested wildcard
        assert!(matcher.matches("docs/intro.md", false));
        assert!(!matcher.matches("docs/other/intro.md", false));
    }

    #[test]
    fn test_ignore_matcher_precedence() {
        let gitignore = b"*.log\n!important.log\nimportant.log";
        // Last one wins.
        let matcher = IgnoreMatcher::new(gitignore);
        assert!(matcher.matches("important.log", false));
    }

    #[test]
    fn test_ignore_matcher_doublestar() {
        let gitignore = b"**/temp";
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("temp", false));
        assert!(matcher.matches("src/temp", false));
        assert!(matcher.matches("a/b/c/temp", false));

        assert!(!matcher.matches("temp/foo", false));
    }

    #[test]
    fn test_ignore_matcher_whitespace() {
        // Trailing spaces are ignored unless escaped
        let gitignore = b"*.txt   \n*.rs\\ ";
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("file.txt", false));
        assert!(matcher.matches("file.rs ", false));
        assert!(!matcher.matches("file.rs", false));
    }

    #[test]
    fn test_ignore_matcher_unicode() {
        let gitignore = "测试/*.log\n!重要.log".as_bytes();
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("测试/error.log", false));
        assert!(!matcher.matches("测试/重要.log", false));
        assert!(!matcher.matches("其他/error.log", false));
    }

    #[test]
    fn test_sort_tree_entries() {
        use gix::objs::tree::Entry;
        use gix::objs::tree::EntryKind;

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
}
