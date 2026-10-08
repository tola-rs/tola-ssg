//! Creation of complete demo sources in a new directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tola_address::{OutputPath, portable_keys_overlap};
use tola_build::cancellation::BuildCancellation;

use super::Demo;
use crate::writes::FileWrites;

pub(crate) fn write(
    demo: &Demo,
    destination: &Path,
    cancellation: &BuildCancellation,
) -> Result<PathBuf> {
    let exported = (|| -> Result<PathBuf> {
        cancellation.ensure_active()?;
        let destination = std::path::absolute(destination)
            .context("Tola could not locate the export directory; choose another path")?;
        match std::fs::symlink_metadata(&destination) {
            Ok(_) => bail!("the export directory already exists; choose a new path"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Tola could not check the export directory"),
        }
        let writes = sources(demo, &destination)?;
        let root = writes.root().to_path_buf();
        writes.apply_new_root(cancellation)?;
        Ok(root)
    })();
    exported.map_err(|error| {
        if error
            .chain()
            .any(|cause| cause.is::<tola_build::cancellation::BuildCancelled>())
        {
            return error;
        }
        let diagnostics =
            crate::cli::output::attached_or_fallback(&error, crate::codes::demo::EXPORT);
        tola_build::diagnostic::DiagnosticError::attach(error, diagnostics).into()
    })
}

pub(super) fn sources(demo: &Demo, root: &Path) -> Result<FileWrites> {
    let mut writes = FileWrites::new(root)?;
    let mut files: Vec<(OutputPath, Vec<String>)> = Vec::new();
    for file in demo.files() {
        let path = OutputPath::parse(file.path)
            .with_context(|| format!("demo source `{}` is not a usable file path", file.path))?;
        let key = path.portable_key();
        if let Some((previous, _)) = files
            .iter()
            .find(|(_, previous)| portable_keys_overlap(previous, &key))
        {
            bail!("demo source `{path}` overlaps `{previous}`");
        }
        writes.create_file(path.as_str(), file.bytes)?;
        files.push((path, key));
    }
    for directory in demo.directories() {
        let path = OutputPath::parse(directory)
            .with_context(|| format!("demo directory `{directory}` is not a usable path"))?;
        let key = path.portable_key();
        if let Some((file, _)) = files
            .iter()
            .find(|(_, file)| key.len() >= file.len() && key.starts_with(file))
        {
            bail!("demo directory `{path}` is below the file `{file}`");
        }
        writes.add_directory(path.as_str())?;
    }
    Ok(writes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occupied_destinations_remain_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        let demo = crate::demos::find("backlinks").unwrap();
        for is_directory in [false, true] {
            let target = parent
                .path()
                .join(if is_directory { "directory" } else { "file" });
            if is_directory {
                std::fs::create_dir(&target).unwrap();
            } else {
                std::fs::write(&target, "Author content").unwrap();
            }
            assert!(write(demo, &target, &BuildCancellation::new()).is_err());
            if is_directory {
                assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
            } else {
                assert_eq!(std::fs::read_to_string(&target).unwrap(), "Author content");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn destination_links_preserve_targets() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("author-directory");
        std::fs::create_dir(&directory).unwrap();
        let demo = crate::demos::find("backlinks").unwrap();
        for existing in [true, false] {
            let target = parent
                .path()
                .join(if existing { "link" } else { "dangling" });
            let points_at = if existing {
                directory.clone()
            } else {
                parent.path().join("missing")
            };
            std::os::unix::fs::symlink(&points_at, &target).unwrap();
            assert!(write(demo, &target, &BuildCancellation::new()).is_err());
            assert!(
                std::fs::symlink_metadata(&target)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
            assert!(!parent.path().join("missing").exists());
        }
    }

    #[test]
    fn export_preserves_complete_sources() {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("export");
        let demo = crate::demos::find("backlinks").unwrap();
        let exported = write(demo, &target, &BuildCancellation::new()).unwrap();
        for file in demo.files() {
            assert_eq!(std::fs::read(exported.join(file.path)).unwrap(), file.bytes);
        }
        for directory in demo.directories() {
            assert!(exported.join(directory).is_dir());
        }
    }
}
