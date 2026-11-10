//! Refresh the facts `tola.lock` records for the inputs a site declared.

use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use tola_build::config::section::IconCollectionSource;
use tola_build::inputs::refresh_module_roots;
use tola_input::{InputLock, InputMode, LockedModule, LockedRevision};
use tola_module::{ModuleDeclaration, ModuleSource, RemoteRevision};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::{ConfigFileArgs, TypstPackageArgs, UpdateArgs};

/// Resolve every selected input again and record what changed.
pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    mode: InputMode,
    update: UpdateArgs,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    ensure!(
        mode.may_write_lock(),
        "`tola update` writes `tola.lock`; run it without `--locked` or `--pure`"
    );
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        mode.scope(),
        &crate::config::ConfigOverrides::default(),
        output,
        cancellation,
    )?;
    let config = loaded.into_config();
    let token = cancellation.token();
    let _guard = tola_build::build::SiteBuildLock::acquire(&config, &token, || {
        let _ = output.waiting_for_build();
    })?;
    let site_root = config.get_root();
    let vendor = config.vendor().path.as_deref();
    let mut lock = InputLock::read(site_root)?.unwrap_or_default();
    let original = lock.clone();

    for name in &update.names {
        ensure!(
            config.modules().contains_key(name) || config.icons().collections.contains_key(name),
            "`{name}` is not a declared module or icon collection"
        );
    }
    let icons = selected_icons(&config.icons().collections, &update.names)?;
    let selected = refreshable(config.modules(), &update.names, &original)?;
    refresh_module_roots(site_root, mode, vendor, &selected, &mut lock, &token)?;
    let resources = crate::cli::config::build_resources(mode);
    tola_build::refresh_icon_collections(&config, &icons, &resources, &token, &mut lock)?;
    token.ensure_active()?;

    for alias in selected.keys() {
        let before = original.modules.get(alias);
        let after = lock.modules.get(alias);
        if before != after {
            output.status(format!("module `{alias}`: {}", describe(after)))?;
        }
    }
    for name in &icons {
        if original.icons.get(name) != lock.icons.get(name) {
            output.status(format!(
                "icon collection `{name}` now has SHA-256 {}",
                lock.icons[name].sha256
            ))?;
        }
    }
    if lock == original {
        output.status("every declared input is already current")?;
        return Ok(());
    }
    if update.dry_run {
        output.status("`--dry-run` wrote no `tola.lock`")?;
        return Ok(());
    }
    token.ensure_active()?;
    lock.commit(site_root, &original)?;
    output.status("`tola.lock` records the refreshed inputs")?;
    output.status("run `tola vendor --refresh` to update the frozen copies")?;
    Ok(())
}

/// The declared modules one update refreshes.
fn refreshable(
    declarations: &BTreeMap<String, ModuleDeclaration>,
    names: &[String],
    lock: &InputLock,
) -> Result<BTreeMap<String, ModuleDeclaration>> {
    let mut selected = BTreeMap::new();
    for (alias, declaration) in declarations {
        if !names.is_empty() && !names.iter().any(|name| name == alias) {
            continue;
        }
        match &declaration.source {
            ModuleSource::Local { .. } => ensure!(
                names.is_empty(),
                "module `{alias}` is a local directory; it is site source and is never refreshed"
            ),
            ModuleSource::Git { git } if pinned(git, lock.modules.get(alias)) => {}
            ModuleSource::Git { .. } | ModuleSource::Tarball { .. } => {
                selected.insert(alias.clone(), declaration.clone());
            }
        }
    }
    Ok(selected)
}

fn pinned(git: &tola_module::declaration::GitSource, locked: Option<&LockedModule>) -> bool {
    let Some(RemoteRevision::Rev(rev)) = &git.revision else {
        return false;
    };
    matches!(locked, Some(LockedModule::Git { git: url, revision: Some(LockedRevision::Rev(recorded)), subdir, commit, .. })
        if url == &git.url && subdir == &git.subdir && recorded == rev && commit == rev)
}

fn selected_icons(
    collections: &BTreeMap<String, IconCollectionSource>,
    names: &[String],
) -> Result<Vec<String>> {
    let mut selected = Vec::new();
    for (name, source) in collections {
        if !names.is_empty() && !names.contains(name) {
            continue;
        }
        ensure!(
            names.is_empty() || matches!(source, IconCollectionSource::RemoteJson { .. }),
            "icon collection `{name}` is local site source and is never refreshed"
        );
        if matches!(source, IconCollectionSource::RemoteJson { .. }) {
            selected.push(name.clone());
        }
    }
    Ok(selected)
}

/// One sentence naming what a module entry now records.
fn describe(module: Option<&LockedModule>) -> String {
    match module {
        Some(LockedModule::Git {
            commit, revision, ..
        }) => {
            let selector = match revision {
                Some(LockedRevision::Tag(tag)) => format!("tag `{tag}`"),
                Some(LockedRevision::Branch(branch)) => format!("branch `{branch}`"),
                Some(LockedRevision::Rev(rev)) => format!("revision `{rev}`"),
                None => "the default branch".to_owned(),
            };
            format!("{selector} now resolves to `{commit}`")
        }
        Some(LockedModule::Tarball { tarball_sha256, .. }) => {
            format!("tarball SHA-256 {tarball_sha256}")
        }
        Some(LockedModule::Local { .. }) => "recorded as a local directory".to_owned(),
        None => "no longer records a fact".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_input::Sha256Digest;
    use tola_module::declaration::GitSource;

    #[test]
    fn fixed_revision_updates_only_when_its_declaration_changes() {
        let commit = "a".repeat(40);
        let mut git = GitSource {
            url: "https://example.com/theme.git".into(),
            subdir: "theme".into(),
            revision: Some(RemoteRevision::Rev(commit.clone())),
        };
        let locked = LockedModule::Git {
            git: git.url.clone(),
            subdir: git.subdir.clone(),
            revision: Some(LockedRevision::Rev(commit.clone())),
            commit,
            tree_sha256: Sha256Digest::of_bytes(b"theme"),
        };
        assert!(!pinned(&git, None));
        assert!(pinned(&git, Some(&locked)));
        git.revision = Some(RemoteRevision::Rev("b".repeat(40)));
        assert!(!pinned(&git, Some(&locked)));
    }
}
