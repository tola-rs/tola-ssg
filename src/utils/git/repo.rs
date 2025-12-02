use crate::{init::init_ignored_files, log};
use anyhow::{Result, anyhow, bail};
use gix::{
    Repository, ThreadSafeRepository,
    commit::NO_PARENT_IDS,
    index::State,
};
use std::{fs, path::Path};

use super::tree::TreeBuilder;

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

/// Get repository root path
pub(crate) fn get_repo_root(repo: &Repository) -> Result<&Path> {
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
