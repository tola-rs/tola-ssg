//! Collect selected release members and their parent directories.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};
use tola_input::{FilePatterns, FileSelection, PatternError};
use tola_typst::BundleCancellation;

use super::{MANIFEST, Member, file_mode};
use tola_fs::is_link_like;

pub(crate) fn validate_selection(pack: &FilePatterns) -> Result<(), PatternError> {
    compile(pack).map(|_| ())
}

pub(super) fn compile(pack: &FilePatterns) -> Result<FileSelection, PatternError> {
    FileSelection::new(pack).map_err(|error| error.at("pack"))
}

pub(super) fn collect(
    sel: &FileSelection,
    root: &Path,
    staging: &Path,
    cancel: &BundleCancellation,
) -> anyhow::Result<BTreeMap<PathBuf, Member>> {
    let mut walk = Walk {
        sel,
        root,
        staging: staging.strip_prefix(root).ok(),
        cancel,
        members: BTreeMap::new(),
    };
    walk.visit(Path::new(""))?;
    for path in sel.literal_includes() {
        ensure!(
            path.as_os_str().is_empty()
                || path == Path::new(MANIFEST)
                || sel.excludes(path)
                || walk.members.contains_key(path),
            "`pack.include` path `{}` was not found",
            path.display()
        );
    }
    Ok(walk.members)
}

struct Walk<'a> {
    sel: &'a FileSelection,
    root: &'a Path,
    staging: Option<&'a Path>,
    cancel: &'a BundleCancellation,
    members: BTreeMap<PathBuf, Member>,
}

impl Walk<'_> {
    fn visit(&mut self, path: &Path) -> anyhow::Result<bool> {
        self.cancel.ensure_active()?;
        if self.staging == Some(path) || path == Path::new(MANIFEST) || self.sel.excludes(path) {
            return Ok(false);
        }
        let selected = self.sel.includes(path);
        if !selected && !self.sel.could_match_below(path) {
            return Ok(false);
        }

        let abs = self.root.join(path);
        let meta = fs::symlink_metadata(&abs)
            .with_context(|| format!("could not read module path `{}`", path.display()))?;
        let link = is_link_like(&meta);
        let member = if meta.is_dir() && !link {
            let mut retained = selected;
            for entry in fs::read_dir(&abs)
                .with_context(|| format!("could not list module directory `{}`", path.display()))?
            {
                let entry = entry.context("could not read a module directory entry")?;
                retained |= self.visit(&path.join(entry.file_name()))?;
            }
            retained.then_some(Member::Directory)
        } else if selected {
            ensure!(
                meta.is_file() && !link,
                "selected member `{}` must be an ordinary file or directory without links",
                path.display()
            );
            Some(Member::File(file_mode(&meta)))
        } else {
            None
        };
        if let Some(member) = member {
            if !path.as_os_str().is_empty() {
                ensure!(
                    path.to_str().is_some()
                        && path
                            .iter()
                            .all(|part| !part.as_encoded_bytes().contains(&b'\\')),
                    "selected member `{}` must have a UTF-8 path with portable separators",
                    path.display()
                );
                self.members.insert(path.to_owned(), member);
            }
            return Ok(true);
        }
        Ok(false)
    }
}
