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

    log!("commit"; "created commit `{commit_id}` in repo `{}`", root.display());
    Ok(())
}

/// Push commits to remote repository
pub fn push(repo: &ThreadSafeRepository, config: &'static SiteConfig) -> Result<()> {
    let github = &config.deploy.github_provider;
    log!("git"; "pushing to `{}`", github.url);

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

#[derive(Debug)]
struct Remote {
    name: String,
    url: String,
}

impl Remote {
    /// Parse remotes from `git remote -v` output
    fn list_from_repo(repo: &Repository) -> Result<Vec<Self>> {
        let root = get_repo_root(repo)?;
        let output = exec!(root; ["git"]; "remote", "-v")?;
        let stdout = std::str::from_utf8(&output.stdout)?;

        let remotes = stdout
            .lines()
            .filter(|line| line.ends_with("(fetch)"))
            .filter_map(Self::parse_remote_line)
            .collect();

        Ok(remotes)
    }

    /// Parse a single remote line: "origin  https://... (fetch)"
    fn parse_remote_line(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        Some(Self {
            name: parts.next()?.to_owned(),
            url: parts.next()?.to_owned(),
        })
    }

    /// Check if origin remote exists with matching URL
    fn origin_matches(repo: &Repository, expected_url: &str) -> Result<bool> {
        Ok(Self::list_from_repo(repo)?
            .iter()
            .any(|r| r.name == "origin" && r.url == expected_url))
    }

    /// Check if origin remote exists
    fn origin_exists(repo: &Repository) -> Result<bool> {
        Ok(Self::list_from_repo(repo)?
            .iter()
            .any(|r| r.name == "origin"))
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

/// Builder for constructing git trees from the filesystem
struct TreeBuilder<'a> {
    repo: &'a ThreadSafeRepository,
    gitignore: &'a [u8],
}

impl<'a> TreeBuilder<'a> {
    fn new(repo: &'a ThreadSafeRepository, gitignore: &'a [u8]) -> Self {
        Self { repo, gitignore }
    }

    /// Build a git tree from a directory
    fn build_from_dir(&self, dir: &Path, index: &mut State) -> Result<Tree> {
        let repo_local = self.repo.to_thread_local();
        let repo_root = self.repo.path().parent().context("Invalid repo path")?;

        let mut entries = Vec::new();

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let filename = self.get_filename(&entry)?;
            let relative_path = path.strip_prefix(repo_root)?.to_string_lossy();

            // Skip ignored and .git directory
            if self.should_ignore(&relative_path, &filename) {
                continue;
            }

            if path.is_dir() {
                let sub_tree = self.build_from_dir(&path, index)?;
                let tree_id = repo_local.write_object(&sub_tree)?.detach();
                entries.push(self.create_tree_entry(filename, tree_id));
            } else if path.is_file() {
                let blob_id = self.write_blob(&repo_local, &path)?;
                self.add_to_index(index, &path, blob_id, &filename)?;
                entries.push(self.create_blob_entry(filename, blob_id));
            }
        }

        // Sort entries according to git tree ordering
        Self::sort_tree_entries(&mut entries);

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
    fn should_ignore(&self, relative_path: &str, filename: &BString) -> bool {
        filename == ".git" || self.matches_gitignore(relative_path)
    }

    /// Check if path matches any gitignore pattern
    fn matches_gitignore(&self, path: &str) -> bool {
        gix::ignore::parse(self.gitignore).any(|(pattern, _, _)| {
            wildmatch(
                path.into(),
                pattern.text.as_bstr(),
                wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
            )
        })
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

    /// Sort entries according to git tree ordering (directories get trailing slash for comparison)
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
