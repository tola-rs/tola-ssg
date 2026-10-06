//! Checked creation and replacement of site files beneath one root.

use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use cap_std::fs::Dir;

#[derive(Debug)]
enum ExpectedContents {
    Missing,
    Exact(Option<Vec<u8>>),
}

#[derive(Debug)]
pub(crate) struct ExistingFile {
    pub(crate) path: PathBuf,
}

impl std::fmt::Display for ExistingFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "file `{}` already exists",
            crate::terminal::display_path(&self.path)
        )
    }
}

impl std::error::Error for ExistingFile {}

#[derive(Debug)]
struct FileWrite {
    path: PathBuf,
    contents: Vec<u8>,
    expected: ExpectedContents,
}

/// Files and directories to write beneath one root.
///
/// Every target is checked before the first write. Replacements capture the
/// actual displaced entry and publish without clobbering a competing rename.
/// Detected conflicts retain that entry in a named backup. This is not strict
/// CAS against editors continuing in-place writes through an already-open handle.
#[derive(Debug)]
pub(crate) struct FileWrites {
    root: PathBuf,
    directories: BTreeSet<PathBuf>,
    files: Vec<FileWrite>,
}

impl FileWrites {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let root = tola_build::filesystem::normalize_existing_prefix(root);
        if !root.is_absolute() {
            bail!(
                "write root `{}` must be absolute",
                crate::terminal::display_path(&root)
            );
        }
        Ok(Self {
            root,
            directories: BTreeSet::new(),
            files: Vec::new(),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Symbolic links and special files are never read. Missing ancestors stay
    /// missing rather than being created.
    pub(crate) fn read_file(&self, relative: impl AsRef<Path>) -> Result<Option<Vec<u8>>> {
        read_target(&self.resolve(relative.as_ref())?)
    }

    pub(crate) fn add_directory(&mut self, relative: impl AsRef<Path>) -> Result<()> {
        let path = self.resolve(relative.as_ref())?;
        self.directories.insert(path);
        Ok(())
    }

    pub(crate) fn create_file(
        &mut self,
        relative: impl AsRef<Path>,
        contents: impl Into<Vec<u8>>,
    ) -> Result<()> {
        let path = self.resolve(relative.as_ref())?;
        self.push_file(path, contents.into(), ExpectedContents::Missing)
    }

    pub(crate) fn replace_file(
        &mut self,
        relative: impl AsRef<Path>,
        contents: impl Into<Vec<u8>>,
    ) -> Result<()> {
        let path = self.resolve(relative.as_ref())?;
        let contents = contents.into();
        let expected = read_target(&path)?;
        if expected.as_deref() == Some(contents.as_slice()) {
            return Ok(());
        }
        self.push_file(path, contents, ExpectedContents::Exact(expected))
    }

    pub(crate) fn replace_file_if_unchanged(
        &mut self,
        relative: impl AsRef<Path>,
        expected: Option<Vec<u8>>,
        contents: impl Into<Vec<u8>>,
    ) -> Result<()> {
        let path = self.resolve(relative.as_ref())?;
        self.push_file(path, contents.into(), ExpectedContents::Exact(expected))
    }

    pub(crate) fn file_paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|file| file.path.as_path())
    }

    pub(crate) fn directory_paths(&self) -> impl Iterator<Item = &Path> {
        self.directories.iter().map(PathBuf::as_path)
    }

    pub(crate) fn relative_path<'a>(&'a self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.root)
            .expect("all writes are contained by their root")
    }

    pub(crate) fn file_contents(&self, relative: &Path) -> Option<&[u8]> {
        let path = self.root.join(relative);
        self.files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.contents.as_slice())
    }

    pub(crate) fn check(&self) -> Result<()> {
        for directory in &self.directories {
            match Directory::open_existing(directory) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "cannot use `{}` as a directory; move the existing path aside or check its permissions",
                            crate::terminal::display_path(directory)
                        )
                    });
                }
            }
        }
        for file in &self.files {
            self.verify_file(file)?;
        }
        Ok(())
    }

    pub(crate) fn apply(
        &self,
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<()> {
        cancellation.ensure_active()?;
        self.check()?;
        let mut created_directories = Vec::new();
        let mut written = Vec::new();
        let result = (|| -> Result<()> {
            self.create_directory(&self.root, &mut created_directories, cancellation)?;
            for directory in &self.directories {
                self.create_directory(directory, &mut created_directories, cancellation)?;
            }
            for file in &self.files {
                cancellation.ensure_active()?;
                let parent = file.path.parent().expect("checked file has a parent");
                // One parent handle at a time: `Directory` pins the ancestor chain, so
                // retaining a handle per written file exhausts the descriptor limit on a
                // scaffold that spans many directories. Rollback reopens its parent.
                let directory =
                    Directory::open(parent, true, &mut created_directories, cancellation)?;
                let name = file
                    .path
                    .file_name()
                    .context("Tola could not write the file: the target is not a file")?;
                let expected = match &file.expected {
                    ExpectedContents::Missing => None,
                    ExpectedContents::Exact(contents) => contents.as_deref(),
                };
                let backup = directory
                    .replace(name, expected, &file.contents, cancellation)
                    .with_context(|| {
                        format!(
                            "cannot write `{}`; review the file and its directory, then retry",
                            crate::terminal::display_path(&file.path)
                        )
                    })?;
                written.push((file, parent.to_path_buf(), backup));
            }
            Ok(())
        })();
        if let Err(mut error) = result {
            for (file, parent, backup) in written.into_iter().rev() {
                let name = file.path.file_name().expect("checked file has a name");
                // Recovery must not depend on cancellation.
                let rolled_back = Directory::open_existing(&parent)
                    .map_err(anyhow::Error::from)
                    .and_then(|directory| directory.rollback(name, &file.contents, backup));
                if let Err(_rollback) = rolled_back {
                    error = error.context(format!(
                        "cannot restore `{}` after a failed write; review the site files, then retry",
                        crate::terminal::display_path(&file.path)
                    ));
                }
            }
            for directory in created_directories.into_iter().rev() {
                if let (Some(parent), Some(name)) = (directory.parent(), directory.file_name())
                    && let Ok(parent) = Directory::open_existing(parent)
                {
                    let _ = parent.remove(name, true);
                }
            }
            return Err(error);
        }
        for (_, _, backup) in written {
            if let Some(mut backup) = backup {
                backup.remove().with_context(|| {
                    format!(
                        "cannot remove committed backup `{}`",
                        crate::terminal::display_path(&backup.directory.path.join(&backup.name))
                    )
                })?;
            }
        }
        Ok(())
    }

    fn verify_file(&self, file: &FileWrite) -> Result<()> {
        let actual = read_target(&file.path)?;
        match &file.expected {
            ExpectedContents::Missing if actual.is_some() => Err(ExistingFile {
                path: file.path.clone(),
            }
            .into()),
            ExpectedContents::Exact(expected) if actual.as_ref() != expected.as_ref() => bail!(
                "file `{}` changed before it could be written; review the current contents and retry",
                crate::terminal::display_path(&file.path)
            ),
            _ => Ok(()),
        }
    }

    fn push_file(
        &mut self,
        path: PathBuf,
        contents: Vec<u8>,
        expected: ExpectedContents,
    ) -> Result<()> {
        if self.files.iter().any(|file| file.path == path) {
            bail!(
                "file `{}` was added more than once",
                crate::terminal::display_path(&path)
            );
        }
        self.files.push(FileWrite {
            path,
            contents,
            expected,
        });
        Ok(())
    }

    fn resolve(&self, relative: &Path) -> Result<PathBuf> {
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            bail!(
                "write path `{}` must be relative to `{}` and must not contain `..`",
                crate::terminal::display_path(relative),
                crate::terminal::display_path(&self.root)
            );
        }
        Ok(self.root.join(relative))
    }

    fn create_directory(
        &self,
        path: &Path,
        created: &mut Vec<PathBuf>,
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<()> {
        Directory::open(path, true, created, cancellation).with_context(|| {
            format!(
                "cannot create safe directory `{}`",
                crate::terminal::display_path(path)
            )
        })?;
        Ok(())
    }
}

pub(crate) fn with_final_newline(mut source: String) -> String {
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source
}

fn read_target(path: &Path) -> Result<Option<Vec<u8>>> {
    let result = (|| -> io::Result<Vec<u8>> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("file has no parent"))?;
        let directory = Directory::open_existing(parent)?;
        directory.read(
            path.file_name()
                .ok_or_else(|| io::Error::other("file has no name"))?,
        )
    })();
    match result {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| {
            format!(
                "cannot safely read `{}`",
                crate::terminal::display_path(path)
            )
        }),
    }
}

/// Unix operations are relative to a no-follow directory descriptor. Windows
/// keeps every ancestor open without delete sharing, pinning the checked path.
#[derive(Clone, Debug)]
struct Directory {
    path: PathBuf,
    _pinned_directories: std::sync::Arc<Vec<Dir>>,
}

impl Directory {
    fn open_existing(path: &Path) -> io::Result<Self> {
        Self::open(path, false, &mut Vec::new(), &Default::default())
    }

    fn open(
        path: &Path,
        create: bool,
        created: &mut Vec<PathBuf>,
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(io::Error::other("directory must be absolute"));
        }
        let mut current = PathBuf::new();
        let mut handles = Vec::new();
        for component in path.components() {
            cancellation.ensure_active().map_err(io::Error::other)?;
            let name = match component {
                std::path::Component::Prefix(_) => {
                    current.push(component);
                    continue;
                }
                std::path::Component::RootDir => {
                    current.push(component);
                    handles.push(crate::sys::open_filesystem_root(&current)?);
                    continue;
                }
                std::path::Component::CurDir => continue,
                std::path::Component::ParentDir => {
                    return Err(io::Error::other("directory must not contain `..`"));
                }
                std::path::Component::Normal(name) => name,
            };
            current.push(name);
            let parent = handles.last().expect("absolute path starts at a root");
            // The refusal covers only the final component: never pass a
            // multi-component path, even for links staying in the root.
            let directory = match crate::sys::open_directory_nofollow(parent, name) {
                Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                    match parent.create_dir(name) {
                        Ok(()) => created.push(current.clone()),
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    crate::sys::open_directory_nofollow(parent, name)?
                }
                result => result?,
            };
            // On Windows cap-std excludes FILE_SHARE_DELETE when opening
            // directories. Keep every verified ancestor alive, not just the
            // final Dir: publication below still needs its pinned native path.
            handles.push(directory);
        }
        Ok(Self {
            path: path.to_owned(),
            _pinned_directories: std::sync::Arc::new(handles),
        })
    }

    fn dir(&self) -> &Dir {
        self._pinned_directories
            .last()
            .expect("directory has a handle")
    }

    fn read(&self, name: &std::ffi::OsStr) -> io::Result<Vec<u8>> {
        let mut file = crate::sys::open_file_for_read(self.dir(), name)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("write target is not a regular file"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn rename(&self, from: &std::ffi::OsStr, to: &std::ffi::OsStr) -> io::Result<()> {
        self.dir().rename(from, self.dir(), to)
    }

    fn publish(&self, from: &std::ffi::OsStr, to: &std::ffi::OsStr) -> io::Result<()> {
        crate::sys::rename_without_replacing(self.dir(), &self.path, from, to)
    }

    fn restore(&self, from: &std::ffi::OsStr, to: &std::ffi::OsStr) -> Result<()> {
        // Keep the captured version until recovery is confirmed. Unlike a hard
        // link, a staged copy also supports Windows FAT/exFAT site roots.
        let staged = self.stage(&self.read(from)?, "tmp")?;
        self.publish(&staged.name, to)?;
        Ok(())
    }

    fn remove(&self, name: &std::ffi::OsStr, directory: bool) -> io::Result<()> {
        if directory {
            self.dir().remove_dir(name)
        } else {
            self.dir().remove_file(name)
        }
    }

    fn stage(&self, contents: &[u8], suffix: &str) -> Result<StoredFile> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| {
            anyhow::anyhow!("Tola could not replace the file; run the command again")
        })?;
        let name = std::ffi::OsString::from(format!(".tola-{}.{suffix}", hex::encode(random)));
        let mut file = crate::sys::create_new_file(self.dir(), &name)?;
        let staged = StoredFile {
            directory: self.clone(),
            name,
            retained: false,
        };
        file.write_all(contents)?;
        Ok(staged)
    }

    fn capture(&self, name: &std::ffi::OsStr) -> Result<StoredFile> {
        let mut saved = self.stage(&[], "backup")?;
        self.rename(name, &saved.name)?;
        // From this point Drop must never erase the displaced version on errors.
        saved.retained = true;
        Ok(saved)
    }

    fn replace(
        &self,
        name: &std::ffi::OsStr,
        expected: Option<&[u8]>,
        contents: &[u8],
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<Option<StoredFile>> {
        let staged = self.stage(contents, "tmp")?;
        cancellation.ensure_active()?;
        let Some(expected) = expected else {
            self.publish(&staged.name, name)?;
            return Ok(None);
        };
        // Moving the actual entry first closes the read-then-overwrite window.
        // Publication uses no-clobber, not a CAS claim against non-cooperating
        // editors: a racing save wins its path and both versions are retained.
        let saved = self.capture(name)?;
        let result = (|| -> Result<()> {
            if self.read(&saved.name)? != expected {
                bail!("file changed before replacement");
            }
            cancellation.ensure_active()?;
            self.publish(&staged.name, name)?;
            Ok(())
        })();
        if let Err(error) = result {
            let restored = self.restore(&saved.name, name);
            return Err(error).with_context(|| {
                format!(
                    "displaced version retained at `{}`; {}",
                    crate::terminal::display_path(&self.path.join(&saved.name)),
                    if restored.is_ok() {
                        "original path restored"
                    } else {
                        "original path was not overwritten; review both versions"
                    },
                )
            });
        }
        Ok(Some(saved))
    }

    fn rollback(
        &self,
        name: &std::ffi::OsStr,
        written: &[u8],
        mut previous: Option<StoredFile>,
    ) -> Result<()> {
        let mut current = self.capture(name).with_context(|| {
            previous.as_ref().map_or_else(
                || "cannot capture current file for rollback".to_owned(),
                |saved| {
                    format!(
                        "earlier version retained at `{}`",
                        crate::terminal::display_path(&self.path.join(&saved.name))
                    )
                },
            )
        })?;
        if self.read(&current.name).ok().as_deref() != Some(written) {
            let _ = self.restore(&current.name, name);
            bail!(
                "concurrent version retained at `{}`; earlier version: {}",
                crate::terminal::display_path(&self.path.join(&current.name)),
                previous.as_ref().map_or_else(
                    || "file was originally absent".to_owned(),
                    |saved| crate::terminal::display_path(&self.path.join(&saved.name))
                )
            );
        }
        if let Some(previous) = previous.as_mut() {
            self.restore(&previous.name, name).with_context(|| {
                format!(
                    "earlier version retained at `{}`",
                    crate::terminal::display_path(&self.path.join(&previous.name))
                )
            })?;
            previous.remove()?;
        }
        current.remove()?;
        Ok(())
    }
}

struct StoredFile {
    directory: Directory,
    name: std::ffi::OsString,
    retained: bool,
}

impl StoredFile {
    fn remove(&mut self) -> io::Result<()> {
        self.directory.remove(&self.name, false)?;
        self.retained = true;
        Ok(())
    }
}

impl Drop for StoredFile {
    fn drop(&mut self) {
        if !self.retained {
            let _ = self.directory.remove(&self.name, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A canonical temporary directory whose handle is opened before any rename, so a
    /// passing assertion proves the checked path stayed bound to the opened entry.
    fn pinned_root() -> (tempfile::TempDir, PathBuf, Directory) {
        let temporary = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(temporary.path()).unwrap();
        let directory = Directory::open_existing(&path).unwrap();
        (temporary, path, directory)
    }

    #[test]
    fn missing_file_read_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let files = FileWrites::new(root.path()).unwrap();
        assert_eq!(files.read_file(".vscode/settings.json").unwrap(), None);
        assert!(!root.path().join(".vscode").exists());
    }

    #[test]
    #[cfg(unix)]
    fn reads_reject_out_of_root_sources() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("settings.json");
        fs::write(&target, "private external contents").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("file-link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("directory-link")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .args(["-m", "600"])
                .arg(root.path().join("pipe"))
                .status()
                .unwrap()
                .success()
        );
        let files = FileWrites::new(root.path()).unwrap();

        assert!(files.read_file("file-link").is_err());
        assert!(files.read_file("directory-link/settings.json").is_err());
        assert!(files.read_file("pipe").is_err());
        assert!(files.read_file("../settings.json").is_err());
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            "private external contents"
        );
    }

    #[test]
    fn publication_keeps_competing_save() {
        let (_temporary, path, directory) = pinned_root();
        let staged = directory.stage(b"ours", "tmp").unwrap();
        fs::write(path.join("settings"), b"editor save").unwrap();
        assert!(
            directory
                .publish(&staged.name, std::ffi::OsStr::new("settings"))
                .is_err()
        );
        assert_eq!(fs::read(path.join("settings")).unwrap(), b"editor save");
        assert_eq!(directory.read(&staged.name).unwrap(), b"ours");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_redirects_nothing() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("templates")).unwrap();
        let mut writes = FileWrites::new(root.path()).unwrap();
        writes.create_file("templates/page.typ", "inside").unwrap();
        assert!(writes.apply(&Default::default()).is_err());
        assert!(!outside.path().join("page.typ").exists());
    }

    #[cfg(unix)]
    #[test]
    fn intermediate_links_are_not_followed() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("real/nested")).unwrap();
        fs::write(root.path().join("real/nested/settings"), b"original").unwrap();
        std::os::unix::fs::symlink("real", root.path().join("alias")).unwrap();
        let mut writes = FileWrites::new(root.path()).unwrap();
        assert!(writes.read_file("alias/nested/settings").is_err());
        writes
            .create_file("alias/nested/created/page.typ", b"ours")
            .unwrap();
        assert!(writes.apply(&Default::default()).is_err());
        assert!(!root.path().join("real/nested/created").exists());
        assert_eq!(
            fs::read(root.path().join("real/nested/settings")).unwrap(),
            b"original"
        );
    }

    #[cfg(windows)]
    #[test]
    fn intermediate_junctions_are_not_followed() {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("real");
        fs::create_dir_all(target.join("nested")).unwrap();
        fs::write(target.join("nested/settings"), b"original").unwrap();
        let junction = root.path().join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cannot create junction: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_ne!(
            fs::symlink_metadata(&junction).unwrap().file_attributes()
                & FILE_ATTRIBUTE_REPARSE_POINT,
            0
        );
        let mut writes = FileWrites::new(root.path()).unwrap();
        assert!(writes.read_file("junction/nested/settings").is_err());
        writes
            .create_file("junction/nested/created/page.typ", b"ours")
            .unwrap();
        assert!(writes.apply(&Default::default()).is_err());
        assert!(!target.join("nested/created").exists());
        assert_eq!(
            fs::read(target.join("nested/settings")).unwrap(),
            b"original"
        );
        fs::remove_dir(junction).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn directory_handle_survives_symlink_swap() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let path = root.join("templates");
        fs::create_dir(&path).unwrap();
        let directory = Directory::open_existing(&path).unwrap();
        fs::rename(&path, root.join("original")).unwrap();
        std::os::unix::fs::symlink(outside.path(), &path).unwrap();
        directory
            .replace(
                std::ffi::OsStr::new("page.typ"),
                None,
                b"inside",
                &Default::default(),
            )
            .unwrap();
        assert_eq!(fs::read(root.join("original/page.typ")).unwrap(), b"inside");
        assert!(!outside.path().join("page.typ").exists());
    }

    #[cfg(unix)]
    #[test]
    fn renamed_ancestors_keep_their_handle() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let path = root.join("parent/nested");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("settings"), b"original").unwrap();
        let directory = Directory::open_existing(&path).unwrap();
        fs::rename(root.join("parent"), root.join("moved")).unwrap();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("settings"), b"unrelated").unwrap();

        let name = std::ffi::OsStr::new("settings");
        let previous = directory
            .replace(name, Some(b"original"), b"ours", &Default::default())
            .unwrap();
        let moved = root.join("moved/nested");
        assert_eq!(fs::read(moved.join(name)).unwrap(), b"ours");
        assert_eq!(fs::read(path.join(name)).unwrap(), b"unrelated");
        directory.rollback(name, b"ours", previous).unwrap();
        assert_eq!(fs::read(moved.join(name)).unwrap(), b"original");
        assert_eq!(fs::read(path.join(name)).unwrap(), b"unrelated");
        assert_eq!(
            fs::read_dir(moved)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            vec![std::ffi::OsString::from("settings")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn ancestors_stay_pinned_until_close() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let path = root.join("parent/nested");
        fs::create_dir_all(&path).unwrap();
        let directory = Directory::open_existing(&path).unwrap();

        assert!(fs::rename(root.join("parent"), root.join("moved")).is_err());
        directory
            .replace(
                std::ffi::OsStr::new("settings"),
                None,
                b"ours",
                &Default::default(),
            )
            .unwrap();
        drop(directory);
        fs::rename(root.join("parent"), root.join("moved")).unwrap();
        assert_eq!(
            fs::read(root.join("moved/nested/settings")).unwrap(),
            b"ours"
        );
    }

    #[test]
    fn conflict_retains_the_displaced_version() {
        let (_temporary, path, directory) = pinned_root();
        fs::write(path.join("settings"), b"editor save").unwrap();
        assert!(
            directory
                .replace(
                    std::ffi::OsStr::new("settings"),
                    Some(b"expected"),
                    b"ours",
                    &Default::default()
                )
                .is_err()
        );
        assert_eq!(fs::read(path.join("settings")).unwrap(), b"editor save");
        let saved = fs::read_dir(&path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|entry| {
                entry
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".tola-")
            })
            .unwrap();
        assert_eq!(fs::read(saved).unwrap(), b"editor save");
    }

    #[test]
    fn rollback_keeps_later_editor_save() {
        let (_temporary, path, directory) = pinned_root();
        fs::write(path.join("settings"), b"original").unwrap();
        let previous = directory
            .replace(
                std::ffi::OsStr::new("settings"),
                Some(b"original"),
                b"ours",
                &Default::default(),
            )
            .unwrap();
        fs::write(path.join("settings"), b"editor save").unwrap();
        assert!(
            directory
                .rollback(std::ffi::OsStr::new("settings"), b"ours", previous)
                .is_err()
        );
        assert_eq!(fs::read(path.join("settings")).unwrap(), b"editor save");
        let versions = fs::read_dir(&path)
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert!(versions.iter().any(|bytes| bytes == b"original"));
        assert!(versions.iter().any(|bytes| bytes == b"editor save"));
    }

    #[test]
    fn cancellation_creates_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("site");
        let mut writes = FileWrites::new(&root).unwrap();
        writes.create_file("site.typ", "Hello").unwrap();
        let canceller = tola_build::cancellation::BuildCanceller::default();
        canceller.cancel();
        let error = writes.apply(&canceller.token()).unwrap_err();
        assert!(error.is::<tola_build::cancellation::BuildCancelled>());
        assert!(!root.exists());
    }

    #[test]
    fn conflict_is_detected_before_writing() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("site");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("second"), "occupied").unwrap();
        let mut writes = FileWrites::new(&root).unwrap();
        writes.create_file("first", "one").unwrap();
        writes.create_file("second", "two").unwrap();

        assert!(
            writes
                .apply(&tola_build::cancellation::BuildCancellation::default())
                .is_err()
        );
        assert!(!root.join("first").exists());
    }

    #[test]
    fn failed_write_restores_every_target() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("settings.toml"), "original").unwrap();
        let mut writes = FileWrites::new(root).unwrap();
        writes.replace_file("settings.toml", "replacement").unwrap();
        writes
            .create_file("generated", "file, not directory")
            .unwrap();
        writes.create_file("generated/child", "unwritten").unwrap();

        assert!(
            writes
                .apply(&tola_build::cancellation::BuildCancellation::new())
                .is_err()
        );
        assert_eq!(fs::read(root.join("settings.toml")).unwrap(), b"original");
        assert!(!root.join("generated").exists());
    }

    #[test]
    fn nested_targets_apply_in_one_pass() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("site");
        let mut writes = FileWrites::new(&root).unwrap();
        writes.add_directory("content/posts").unwrap();
        writes
            .create_file("content/posts/index.typ", "hello")
            .unwrap();

        writes
            .apply(&tola_build::cancellation::BuildCancellation::default())
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("content/posts/index.typ")).unwrap(),
            "hello"
        );
    }
}
