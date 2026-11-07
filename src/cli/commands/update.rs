//! Refresh the facts `tola.lock` records for the inputs a site declared.

use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use tola_build::inputs::{InputLock, InputMode, LockedModule, LockedRevision, record_module_roots};
use tola_build::modules::{ModuleDeclaration, ModuleSource, RemoteRevision};

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
        "`tola update` writes `tola.lock`; run it without `--locked`, `--offline`, or `--pure`"
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
    let site_root = config.get_root();
    let vendor = config.vendor().path.as_deref();
    let mut lock = InputLock::read(site_root)?.unwrap_or_default();
    let original = lock.clone();

    let selected = refreshable(config.modules(), &update.names, output)?;
    for alias in selected.keys() {
        // Dropping the fact is what makes the next resolution fetch the selector again.
        lock.modules.remove(alias);
    }
    record_module_roots(
        site_root,
        mode,
        vendor,
        &selected,
        &mut lock,
        &cancellation.token(),
    )?;

    for alias in selected.keys() {
        let before = original.modules.get(alias);
        let after = lock.modules.get(alias);
        if before != after {
            output.status(format!("module `{alias}`: {}", describe(after)))?;
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
    lock.write_atomic(site_root)?;
    output.status("`tola.lock` records the refreshed inputs")?;
    output.status("run `tola vendor` to update the frozen copies")?;
    Ok(())
}

/// The declared modules one update refreshes.
fn refreshable(
    declarations: &BTreeMap<String, ModuleDeclaration>,
    names: &[String],
    output: &CommandOutput,
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
            ModuleSource::Remote {
                revision: Some(RemoteRevision::Rev(_)),
                ..
            } => {
                if !names.is_empty() {
                    output.status(format!(
                        "module `{alias}` is pinned to a revision; change it to `tag` or `branch` to refresh it"
                    ))?;
                }
            }
            ModuleSource::Remote { .. } => {
                selected.insert(alias.clone(), declaration.clone());
            }
        }
    }
    for name in names {
        ensure!(
            declarations.contains_key(name),
            "`{name}` is not a declared module"
        );
    }
    Ok(selected)
}

/// One sentence naming what a module entry now records.
fn describe(module: Option<&LockedModule>) -> String {
    match module {
        Some(LockedModule::Remote {
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
        Some(LockedModule::Local { .. }) => "recorded as a local directory".to_owned(),
        None => "no longer records a fact".to_owned(),
    }
}
