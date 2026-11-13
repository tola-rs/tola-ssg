//! Refresh the facts `tola.lock` records for the inputs a site declared.

use std::collections::BTreeMap;

use anyhow::{Error, Result, ensure};
use tola_build::config::section::IconCollectionSource;
use tola_build::inputs::refresh_module_roots;
use tola_diagnostic::DiagnosticError;
use tola_input::{
    InputLock, InputMode, LockedModule, LockedRevision, Sha256Digest, TarballLocation,
};
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
    refresh_module_roots(
        site_root,
        mode,
        vendor,
        &selected,
        &mut lock,
        &token,
        |alias, source| match source {
            ModuleSource::Git { git } => {
                let selector = match &git.revision {
                    Some(RemoteRevision::Tag(tag)) => format!("tag {tag}"),
                    Some(RemoteRevision::Branch(branch)) => format!("branch {branch}"),
                    Some(RemoteRevision::Rev(rev)) => format!("commit {}", short_commit(rev)),
                    None => "default branch".into(),
                };
                let verb = if mode.may_use_network() {
                    "Fetching"
                } else {
                    "Checking"
                };
                let _ = status(output, verb, format_args!("{alias} ({selector})"));
            }
            ModuleSource::Tarball { tarball } => {
                let (verb, archive) = match &tarball.location {
                    TarballLocation::Url { url } => {
                        let address = url.split(['?', '#']).next().unwrap_or(url);
                        let file = address
                            .splitn(4, '/')
                            .nth(3)
                            .and_then(|path| path.trim_end_matches('/').rsplit('/').next())
                            .filter(|file| !file.is_empty())
                            .unwrap_or("archive");
                        let verb = if mode.may_use_network() {
                            "Downloading"
                        } else {
                            "Checking"
                        };
                        (verb, file.to_owned())
                    }
                    TarballLocation::File { path } => (
                        "Reading",
                        tola_diagnostic::display_path(&site_root.join(path), site_root),
                    ),
                };
                let _ = status(output, verb, format_args!("{alias} ({archive})"));
            }
            ModuleSource::Local { .. } => {}
        },
    )
    .map_err(|error| refresh_failed(error.into()))?;
    let resources = crate::cli::config::build_resources(mode);
    tola_build::refresh_icon_collections(&config, &icons, &resources, &token, &mut lock, |name| {
        let verb = if mode.may_use_network() {
            "Fetching"
        } else {
            "Checking"
        };
        let _ = status(output, verb, format_args!("{name} (icon collection)"));
    })
    .map_err(|error| refresh_failed(error.into()))?;
    token.ensure_active()?;

    let mut pinned_count = 0;
    for (alias, declaration) in config.modules() {
        if !update.names.contains(alias) {
            continue;
        }
        if let ModuleSource::Git { git } = &declaration.source
            && let Some(RemoteRevision::Rev(rev)) = &git.revision
            && pinned(git, original.modules.get(alias))
        {
            status(
                output,
                "Pinned",
                format_args!("{alias}  {}", short_commit(rev)),
            )?;
            pinned_count += 1;
        }
    }

    let checked = selected.len() + icons.len();
    if lock == original {
        if checked == 1 && !update.names.is_empty() {
            if let Some(alias) = selected.keys().next() {
                status(
                    output,
                    "Unchanged",
                    format_args!("{alias}  {}", module_value(&lock.modules[alias])),
                )?;
            } else {
                let name = &icons[0];
                status(
                    output,
                    "Unchanged",
                    format_args!(
                        "{name} (icon collection)  {}",
                        short_digest(lock.icons[name].sha256)
                    ),
                )?;
            }
        } else if checked > 0 {
            status(
                output,
                "Unchanged",
                crate::terminal::progress::plural_count(checked, "input"),
            )?;
        } else if pinned_count == 0 {
            output.status("No inputs to refresh")?;
        }
        return dry_run_outcome(output, update.dry_run);
    }

    if !update.dry_run {
        token.ensure_active()?;
        if !lock
            .commit(site_root, &original)
            .map_err(|error| refresh_failed(error.into()))?
        {
            output.status("`tola.lock` already records the refreshed inputs")?;
            return Ok(());
        }
    }
    let verb = if update.dry_run {
        "Would update"
    } else {
        "Updated"
    };
    let mut changed = 0;
    for alias in selected.keys() {
        let before = original.modules.get(alias);
        let after = &lock.modules[alias];
        if before != Some(after) {
            status(
                output,
                verb,
                format_args!("{alias}  {}", module_change(before, after)),
            )?;
            changed += 1;
        }
    }
    for name in &icons {
        let after = &lock.icons[name];
        let before = original.icons.get(name);
        if before != Some(after) {
            let detail = match before {
                Some(before) if before.sha256 != after.sha256 => {
                    format!("sha256:{}", digest_change(before.sha256, after.sha256))
                }
                Some(_) => "source changed".into(),
                None => short_digest(after.sha256),
            };
            status(
                output,
                verb,
                format_args!("{name} (icon collection)  {detail}"),
            )?;
            changed += 1;
        }
    }
    if !update.dry_run {
        status(
            output,
            "Wrote",
            format_args!(
                "tola.lock ({changed} changed, {} unchanged)",
                checked - changed
            ),
        )?;
        if vendor.is_some() {
            output.status("Run `tola vendor --refresh` to update the frozen copies")?;
        }
    }
    dry_run_outcome(output, update.dry_run)
}

fn dry_run_outcome(output: &CommandOutput, dry_run: bool) -> Result<()> {
    if dry_run {
        output
            .status("`--dry-run` leaves `tola.lock` untouched and may populate the input cache")?;
    }
    Ok(())
}

fn status(
    output: &CommandOutput,
    verb: &str,
    detail: impl std::fmt::Display,
) -> std::io::Result<()> {
    output.status(format!("{verb:>12} {detail}"))
}

fn refresh_failed(error: Error) -> Error {
    if tola_build::cancellation::is_cancelled(&error) {
        return error;
    }
    let mut diagnostics = match tola_module::source_diagnostic(&error) {
        Some(diagnostic) => vec![diagnostic],
        None => crate::cli::output::attached_or_fallback(&error, crate::codes::command::FAILED),
    };
    if let Some(diagnostic) = diagnostics.first_mut() {
        diagnostic
            .notes
            .push("this update did not write `tola.lock`".into());
    }
    DiagnosticError::attach(error, diagnostics).into()
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

fn module_value(module: &LockedModule) -> String {
    match module {
        LockedModule::Git { commit, .. } => short_commit(commit).into(),
        LockedModule::Tarball { tarball_sha256, .. } => short_digest(*tarball_sha256),
        LockedModule::Local { .. } => "local directory".into(),
    }
}

fn module_change(before: Option<&LockedModule>, after: &LockedModule) -> String {
    let mut changes = Vec::new();
    match (before, after) {
        (
            Some(LockedModule::Git {
                git,
                subdir,
                revision,
                commit,
                tree_sha256,
            }),
            LockedModule::Git {
                git: new_git,
                subdir: new_subdir,
                revision: new_revision,
                commit: new_commit,
                tree_sha256: new_tree,
            },
        ) => {
            if commit != new_commit {
                let (old, new) = (short_commit(commit), short_commit(new_commit));
                changes.push(if old == new {
                    format!("{commit} → {new_commit}")
                } else {
                    format!("{old} → {new}")
                });
            }
            if git != new_git {
                changes.push("git.url changed".into());
            }
            if revision != new_revision {
                changes.push(format!(
                    "ref: {} → {}",
                    locked_selector(revision),
                    locked_selector(new_revision)
                ));
            }
            if subdir != new_subdir {
                let old = if subdir.as_os_str().is_empty() {
                    ".".into()
                } else {
                    subdir.display().to_string()
                };
                let new = if new_subdir.as_os_str().is_empty() {
                    ".".into()
                } else {
                    new_subdir.display().to_string()
                };
                changes.push(format!("subdir: {old} → {new}"));
            }
            if changes.is_empty() && tree_sha256 != new_tree {
                changes.push(format!(
                    "tree SHA-256: {}",
                    digest_change(*tree_sha256, *new_tree)
                ));
            }
        }
        (
            Some(LockedModule::Tarball {
                tarball,
                tarball_sha256,
                tree_sha256,
            }),
            LockedModule::Tarball {
                tarball: new_tarball,
                tarball_sha256: new_digest,
                tree_sha256: new_tree,
            },
        ) => {
            if tarball_sha256 != new_digest {
                changes.push(format!(
                    "sha256:{}",
                    digest_change(*tarball_sha256, *new_digest)
                ));
            }
            if tarball != new_tarball {
                changes.push("archive source changed".into());
            }
            if changes.is_empty() && tree_sha256 != new_tree {
                changes.push(format!(
                    "tree SHA-256: {}",
                    digest_change(*tree_sha256, *new_tree)
                ));
            }
        }
        (Some(before), _) => return format!("{} → {}", module_value(before), module_value(after)),
        (None, _) => return format!("locked to {}", module_value(after)),
    }
    changes.join(", ")
}

fn locked_selector(revision: &Option<LockedRevision>) -> String {
    match revision {
        Some(LockedRevision::Tag(tag)) => format!("tag {tag}"),
        Some(LockedRevision::Branch(branch)) => format!("branch {branch}"),
        Some(LockedRevision::Rev(rev)) => format!("commit {}", short_commit(rev)),
        None => "default branch".into(),
    }
}

fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

fn short_digest(digest: Sha256Digest) -> String {
    format!("sha256:{}…", &digest.to_string()[..8])
}

fn digest_change(before: Sha256Digest, after: Sha256Digest) -> String {
    let (before, after) = (before.to_string(), after.to_string());
    if before[..8] == after[..8] {
        format!("{before} → {after}")
    } else {
        format!("{}… → {}…", &before[..8], &after[..8])
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
