//! Site deployment module.
//!
//! Handles deployment to various hosting providers.

use crate::{config::SiteConfig, utils::git};
use anyhow::{Result, bail};

/// Deploy the built site to configured provider
pub fn deploy_site(config: &SiteConfig) -> Result<()> {
    match config.deploy.provider.as_str() {
        "github" => deploy_github(config),
        _ => bail!("This platform is not supported now"),
    }
}

/// Deploy to GitHub Pages
fn deploy_github(config: &SiteConfig) -> Result<()> {
    let repo = ensure_output_repo(config)?;

    git::commit_all(&repo, "deploy it")?;
    git::push(&repo, config)?;
    Ok(())
}

/// Ensure output directory is a git repository for deploy
fn ensure_output_repo(config: &SiteConfig) -> Result<gix::ThreadSafeRepository> {
    git::open_repo(&config.build.output).or_else(|_| git::create_repo(&config.build.output))
}
