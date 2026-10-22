use super::{ModuleDeclaration, ModuleSource, RemoteRevision, remote, valid_name};
use crate::filesystem::{INTERNAL_DIR, layout};
use crate::{InputScope, cancellation::BuildCancellation};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleLock {
    #[serde(default)]
    modules: BTreeMap<String, LockedModule>,
}

#[derive(Serialize, Deserialize)]
struct LockedModule {
    repository: String,
    #[serde(flatten)]
    revision: RemoteRevision,
    subdir: PathBuf,
    commit: String,
}

fn read_lock(site: &Path) -> Result<ModuleLock> {
    match std::fs::read_to_string(site.join("tola.lock")) {
        Ok(text) => {
            let mut lock: ModuleLock = toml::from_str(&text)?;
            for module in lock.modules.values_mut() {
                module.commit = gix::ObjectId::from_hex(module.commit.as_bytes())
                    .context("`tola.lock` records an invalid commit")?
                    .to_string();
            }
            Ok(lock)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ModuleLock::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn declared_root(site: &Path, alias: &str, source: &ModuleSource) -> Result<PathBuf> {
    match source {
        ModuleSource::Local { path } => Ok(site.join(path)),
        ModuleSource::Remote { .. } => module_root(site, alias, source),
    }
}

pub(super) fn module_root(site: &Path, alias: &str, source: &ModuleSource) -> Result<PathBuf> {
    let ModuleSource::Remote {
        repository,
        revision,
        subdir,
    } = source
    else {
        anyhow::bail!("not a remote module")
    };
    let lock = read_lock(site)?;
    let module = lock.modules.get(alias).with_context(|| {
        format!("module `{alias}` is not locked; run `tola module fetch {alias}`")
    })?;
    ensure!(
        valid_name(alias)
            && module.repository == *repository
            && module.revision == *revision
            && module.subdir == *subdir,
        "the declaration of module `{alias}` changed since it was fetched; run `tola module fetch {alias}`"
    );
    let snapshot = layout::modules(site).join(alias).join(&module.commit);
    let root = snapshot.join(subdir).canonicalize()?;
    ensure!(
        root.starts_with(snapshot.canonicalize()?) && root.is_dir(),
        "the directory of module `{alias}` points outside its fetched revision"
    );
    Ok(root)
}

pub fn fetch_modules(
    site: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    alias: Option<&str>,
    scope: InputScope,
    cancellation: &BuildCancellation,
) -> Result<()> {
    ensure!(
        scope == InputScope::Online,
        "`tola module fetch` requires network access; remove `--offline` or `--pure`"
    );
    if let Some(alias) = alias {
        ensure!(
            declarations.contains_key(alias),
            "module `{alias}` is not declared in `modules`"
        );
        ensure!(
            matches!(declarations[alias].source, ModuleSource::Remote { .. }),
            "module `{alias}` has no repository to fetch"
        );
    }
    for (name, declaration) in declarations {
        if alias.is_some_and(|selected| selected != name) {
            continue;
        }
        let ModuleSource::Remote {
            repository,
            revision,
            subdir,
        } = &declaration.source
        else {
            continue;
        };
        ensure!(valid_name(name), "`{name}` is not a usable module name");
        if matches!(revision, RemoteRevision::Rev(_))
            && module_root(site, name, &declaration.source).is_ok()
        {
            continue;
        }
        cancellation.ensure_active()?;
        let parent = layout::modules(site).join(name);
        std::fs::create_dir_all(&parent)?;
        let stage = tempfile::tempdir_in(site.join(INTERNAL_DIR))?;
        let fetched = remote::fetch(repository, revision, stage.path(), cancellation)
            .with_context(|| format!("Tola could not fetch module `{name}`"))?;
        let root = fetched.root.join(subdir).canonicalize().with_context(|| {
            format!("the `subdir` of module `{name}` does not exist in the fetched revision")
        })?;
        ensure!(
            root.starts_with(fetched.root.canonicalize()?) && root.is_dir(),
            "the `subdir` of module `{name}` points outside the fetched revision"
        );
        let mut staged = declaration.clone();
        staged.source = ModuleSource::Local { path: root };
        super::resolve_modules(
            site,
            &BTreeMap::from([(name.clone(), staged)]),
            InputScope::Online,
        )
        .with_context(|| format!("the fetched copy of module `{name}` is not usable"))?;
        cancellation.ensure_active()?;
        let destination = parent.join(&fetched.commit);
        if !destination.exists() {
            std::fs::rename(&fetched.root, &destination)?;
        }
        let _site_lock = crate::build::SiteBuildLock::acquire_at_root(site, cancellation, || {})?;
        let mut lock = read_lock(site).context("Tola could not read `tola.lock`")?;
        lock.modules.insert(
            name.clone(),
            LockedModule {
                repository: repository.clone(),
                revision: revision.clone(),
                subdir: subdir.clone(),
                commit: fetched.commit,
            },
        );
        let mut temporary = tempfile::NamedTempFile::new_in(site)?;
        use std::io::Write;
        temporary.write_all(toml::to_string_pretty(&lock)?.as_bytes())?;
        cancellation.ensure_active()?;
        temporary
            .persist(site.join("tola.lock"))
            .map_err(|error| error.error)?;
    }
    Ok(())
}
