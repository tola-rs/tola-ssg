//! Prepare and publish the files selected by a module author.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::{ErrorKind, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use tola_fs::{
    DirectoryReplacement, ReplaceDirectoryError, is_link_like, open_regular_file, set_unix_mode,
};
use tola_input::FilesystemSourceIdentity;
use tola_typst::BundleCancellation;

use crate::{Action, check};

mod selection;

pub(crate) use selection::validate_selection;

const MANIFEST: &str = "tola-module.toml";

/// Run author hooks, check the release directory, and replace the destination.
pub fn pack(
    source: &Path,
    destination: &Path,
    cancellation: &BundleCancellation,
    mut run_hook: impl FnMut(&str, &Action, &Path, &Path) -> Result<()>,
) -> Result<()> {
    cancellation.ensure_active()?;
    let loaded = check::read(source)?;
    let root = &loaded.root;
    let pack = loaded
        .manifest
        .pack
        .as_ref()
        .context("`tola-module.toml` needs `[pack]` with an `include` list")?;
    let selection = selection::compile(pack)?;
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    let shown = tola_diagnostic::display_path(destination, &cwd);
    let target = target_path(destination, &shown)?;
    ensure!(
        !FilesystemSourceIdentity::from_path(&root.join(MANIFEST)).intersects_path(&target)
            && !target
                .strip_prefix(root)
                .is_ok_and(|path| selection.could_match_below(path)),
        "pack target `{shown}` overlaps the selected source"
    );

    let mut bytes = Vec::new();
    open_member(root, Path::new(MANIFEST))?
        .read_to_end(&mut bytes)
        .context("could not read `tola-module.toml`")?;
    ensure!(
        blake3::hash(&bytes) == loaded.evidence.digest,
        "`tola-module.toml` changed while Tola was reading it"
    );
    let mut manifest: toml::Table = toml::from_str(std::str::from_utf8(&bytes)?)?;
    manifest.remove("pack");
    if let Some(hooks) = manifest
        .get_mut("hooks")
        .and_then(toml::Value::as_table_mut)
    {
        hooks.remove("before-pack");
        if hooks.is_empty() {
            manifest.remove("hooks");
        }
    }
    let manifest = toml::to_string(&manifest).context("could not write the release manifest")?;

    let stage = DirectoryReplacement::new(&target)
        .with_context(|| format!("could not prepare pack target `{shown}`"))?;

    for hook in loaded
        .manifest
        .hooks
        .before_pack
        .iter()
        .filter(|hook| hook.enable)
    {
        cancellation.ensure_active()?;
        let key = format!("hooks.before-pack ({}).nu", hook.name);
        let action = hook.action().map_err(anyhow::Error::msg)?;
        check::check_action(root, &key, &action)?;
        let temp =
            tempfile::tempdir().context("could not prepare the before-pack temporary directory")?;
        run_hook(&hook.name, &action, root, temp.path())?;
    }
    cancellation.ensure_active()?;
    ensure!(
        loaded.evidence.is_current(cancellation)?,
        "`tola-module.toml` or its source directory changed during before-pack hooks"
    );
    ensure!(
        target_path(destination, &shown)? == target,
        "pack target `{shown}` moved while packing"
    );

    let mut members = selection::collect(&selection, root, stage.workspace(), cancellation)?;
    // The author's manifest is replaced even when it was explicitly selected.
    members.insert(PathBuf::from(MANIFEST), Member::File(0o644));
    let target_key = target.strip_prefix(root).ok().map(portable_key);
    let mut portable_paths = BTreeMap::new();
    for path in members.keys() {
        let name = tola_diagnostic::render_relative(path);
        let key = portable_key(path);
        ensure!(
            target_key.as_ref() != Some(&key),
            "pack target `{shown}` overlaps selected member `{name}`"
        );
        if let Some(other) = portable_paths.insert(key, path) {
            bail!(
                "selected paths `{}` and `{name}` overlap on another filesystem",
                other.display()
            );
        }
    }

    let release = stage.staged();
    fs::create_dir(&release).context("could not create the release directory")?;
    set_unix_mode(&release, 0o755).context("could not set release directory permissions")?;
    for (path, member) in &members {
        cancellation.ensure_active()?;
        stage_member(root, &release, path, *member, &manifest)
            .with_context(|| format!("could not stage `{}`", path.display()))?;
    }
    check::check(&release)?;

    ensure!(
        target_path(destination, &shown)? == target,
        "pack target `{shown}` moved while packing"
    );
    cancellation.ensure_active()?;
    publish_directory(stage, &shown)?;
    Ok(())
}

fn portable_key(path: &Path) -> Vec<String> {
    tola_diagnostic::render_relative(path)
        .split('/')
        .map(tola_address::portable_collision_key)
        .collect()
}

fn target_path(destination: &Path, shown: &str) -> Result<PathBuf> {
    let path = destination;
    let name = path
        .file_name()
        .with_context(|| format!("invalid pack target `{shown}`"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent
        .canonicalize()
        .with_context(|| format!("could not read the parent directory of pack target `{shown}`"))?;
    ensure!(
        parent.is_dir(),
        "pack target `{shown}` needs a parent directory"
    );
    let target = parent.join(name);
    match fs::symlink_metadata(&target) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && !is_link_like(&metadata),
            "pack target `{shown}` must be absent or an ordinary directory"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("could not read pack target `{shown}`"));
        }
    }
    Ok(target)
}

#[derive(Clone, Copy)]
enum Member {
    Directory,
    File(u32),
}

fn member_metadata(root: &Path, path: &Path) -> Result<Metadata> {
    let absolute = root.join(path);
    let metadata = fs::symlink_metadata(&absolute)
        .with_context(|| format!("could not read selected member `{}`", path.display()))?;
    ensure!(
        (metadata.is_dir() || metadata.is_file()) && !is_link_like(&metadata),
        "selected member `{}` must be an ordinary file or directory without links",
        path.display()
    );
    Ok(metadata)
}

fn stage_member(
    root: &Path,
    release: &Path,
    path: &Path,
    member: Member,
    manifest: &str,
) -> Result<()> {
    let target = release.join(path);
    match member {
        Member::Directory => {
            fs::create_dir(&target)?;
            set_unix_mode(&target, 0o755)?;
        }
        Member::File(mode) => {
            if path == Path::new(MANIFEST) {
                File::create_new(&target)?.write_all(manifest.as_bytes())?;
            } else {
                // Check intermediate directories again before opening the selected file.
                for parent in path
                    .ancestors()
                    .skip(1)
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    ensure!(
                        member_metadata(root, parent)?.is_dir(),
                        "`{}` must be a directory",
                        parent.display()
                    );
                }
                let mut source = open_member(root, path)?;
                let mut file = File::create_new(&target)?;
                std::io::copy(&mut source, &mut file)?;
            }
            set_unix_mode(&target, mode)?;
        }
    }
    Ok(())
}

fn open_member(root: &Path, path: &Path) -> Result<File> {
    let file = open_regular_file(&root.join(path), false)
        .with_context(|| format!("could not open `{}` as an ordinary file", path.display()))?;
    ensure!(
        tola_fs::link_count(&file)? <= 1,
        "selected member `{}` cannot be a hard link",
        path.display()
    );
    Ok(file)
}

fn file_mode(metadata: &Metadata) -> u32 {
    if tola_fs::unix_mode(metadata).is_some_and(|mode| mode & 0o111 != 0) {
        0o755
    } else {
        0o644
    }
}

fn publish_directory(stage: DirectoryReplacement, shown: &str) -> Result<()> {
    match stage.publish() {
        Ok(()) => Ok(()),
        Err(ReplaceDirectoryError::Restore { previous, .. }) => {
            let cwd = std::env::current_dir()?;
            bail!(
                "could not replace pack target `{shown}`; restore the previous release from `{}`",
                tola_diagnostic::display_path(&previous, &cwd)
            );
        }
        Err(error) => {
            Err(error).with_context(|| format!("could not publish pack target `{shown}`"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_removes_stale_files_and_keeps_the_last_valid_release() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("source");
        let target = workspace.path().join("release");
        fs::create_dir(&source).unwrap();
        fs::write(source.join(MANIFEST), "[typst]\nentry = 'lib.typ'\n[pack]\ninclude = ['lib.typ']\n[[hooks.before-pack]]\nname = 'generate'\nargv = ['generate']\n").unwrap();
        let cancellation = BundleCancellation::default();
        let generate = |_: &str, _: &Action, root: &Path, _: &Path| {
            fs::write(root.join("lib.typ"), "#let value = 1")?;
            Ok(())
        };
        pack(&source, &target, &cancellation, generate).unwrap();
        let manifest: toml::Table =
            toml::from_str(&fs::read_to_string(target.join(MANIFEST)).unwrap()).unwrap();
        assert!(!manifest.contains_key("pack"));
        assert!(!manifest.contains_key("hooks"));
        fs::write(target.join("stale.txt"), "old release").unwrap();
        pack(&source, &target, &cancellation, generate).unwrap();
        assert!(!target.join("stale.txt").exists());
        let digest = tola_input::tree_sha256(&target).unwrap();
        fs::remove_file(source.join("lib.typ")).unwrap();
        assert!(pack(&source, &target, &cancellation, |_, _, _, _| Ok(())).is_err());
        assert_eq!(tola_input::tree_sha256(&target).unwrap(), digest);
        check::check(&target).unwrap();
    }

    #[test]
    fn selected_target_is_refused_before_author_hooks() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join(MANIFEST), "[pack]\ninclude = ['**/*.css']\n[[hooks.before-pack]]\nname = 'compile'\nargv = ['compile']\n").unwrap();
        let target = source.path().join("release");
        let cancellation = BundleCancellation::default();
        assert!(
            pack(source.path(), &target, &cancellation, |_, _, _, _| {
                panic!("an overlapping target must be refused before hooks run")
            })
            .is_err()
        );
        assert!(!target.exists());
    }
}
