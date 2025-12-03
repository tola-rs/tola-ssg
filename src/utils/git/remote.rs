use crate::{config::SiteConfig, exec, log};
use anyhow::{Context, Result, bail};
use gix::{
    Repository, ThreadSafeRepository,
    remote::Direction,
};
use std::{fs, path::Path};

use super::repo::get_repo_root;

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

struct Remote;

impl Remote {
    /// Check if origin remote exists with matching URL
    fn origin_matches(repo: &Repository, expected_url: &str) -> Result<bool> {
        let matches = repo
            .find_remote("origin")
            .ok()
            .and_then(|remote| {
                remote
                    .url(Direction::Push)
                    .or_else(|| remote.url(Direction::Fetch))
                    .map(|url| url.to_bstring() == expected_url)
            })
            .unwrap_or(false);
        Ok(matches)
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
    exec!(pty=true; root; ["git"]; "remote", action, "origin", url)?;
    Ok(())
}

/// Push to remote with optional force flag
fn push_to_remote(root: &Path, branch: &str, force: bool) -> Result<()> {
    if force {
        exec!(pty=true; root; ["git"]; "push", "--set-upstream", "origin", branch, "-f")?;
    } else {
        exec!(pty=true; root; ["git"]; "push", "--set-upstream", "origin", branch)?;
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
    fn test_remote_origin_exists() {
        with_temp_dir(|dir| {
            // 1. Init repo
            {
                let _repo = gix::init(dir).unwrap();
            } // Drop repo to release any locks

            // 2. Add remote using git command
            let status = std::process::Command::new("git")
                .args(["remote", "add", "origin", "https://example.com/repo.git"])
                .current_dir(dir)
                .status()
                .expect("Failed to execute git command");

            assert!(status.success(), "git remote add failed");

            // 3. Re-open repo and check
            let repo = gix::open(dir).unwrap();

            assert!(Remote::origin_exists(&repo).unwrap());
            assert!(Remote::origin_matches(&repo, "https://example.com/repo.git").unwrap());
            assert!(!Remote::origin_matches(&repo, "https://other.com/repo.git").unwrap());
        });
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
}
