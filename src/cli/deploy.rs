//! Site deployment module.
//!
//! Handles deployment to various hosting providers.

use crate::{config::SiteConfig, utils::git};
use anyhow::{Result, bail};
use gix::ThreadSafeRepository;

/// Deploy the built site to configured provider
pub fn deploy_site(repo: &ThreadSafeRepository, config: &SiteConfig) -> Result<()> {
    match config.deploy.provider.as_str() {
        "github" => deploy_github(repo, config),
        _ => bail!("This platform is not supported now"),
    }
}

/// Deploy to GitHub Pages
fn deploy_github(repo: &ThreadSafeRepository, config: &SiteConfig) -> Result<()> {
    git::commit_all(repo, "deploy it")?;
    git::push(repo, config)?;
    Ok(())
}
