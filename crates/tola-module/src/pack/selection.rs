//! Collect selected release members and their parent directories.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use tola_input::{FilePatterns, FileSelection};
use tola_typst::BundleCancellation;

use super::{MANIFEST, Member, file_mode};
use tola_fs::is_link_like;

pub(crate) fn validate_selection(pack: &FilePatterns) -> Result<()> {
    compile(pack).map(|_| ())
}

pub(super) fn compile(pack: &FilePatterns) -> Result<FileSelection> {
    FileSelection::new(pack).map_err(|error| error.at("pack").into())
}

pub(super) fn overlaps_target(selection: &FileSelection, path: &Path) -> bool {
    !selection.excludes(path)
        && (selection.includes(path)
            || selection
                .literal_includes()
                .any(|literal| literal.starts_with(path) && !selection.excludes(literal)))
}

pub(super) fn collect(
    selection: &FileSelection,
    root: &Path,
    staging: &Path,
    cancellation: &BundleCancellation,
) -> Result<BTreeMap<PathBuf, Member>> {
    let mut walk = Walk {
        selection,
        root,
        staging: staging.strip_prefix(root).ok(),
        cancellation,
        members: BTreeMap::new(),
    };
    walk.visit(Path::new(""))?;
    for path in selection.literal_includes() {
        ensure!(
            path.as_os_str().is_empty()
                || path == Path::new(MANIFEST)
                || selection.excludes(path)
                || walk.members.contains_key(path),
            "`pack.include` path `{}` was not found",
            path.display()
        );
    }
    Ok(walk.members)
}

struct Walk<'a> {
    selection: &'a FileSelection,
    root: &'a Path,
    staging: Option<&'a Path>,
    cancellation: &'a BundleCancellation,
    members: BTreeMap<PathBuf, Member>,
}

impl Walk<'_> {
    fn visit(&mut self, path: &Path) -> Result<bool> {
        self.cancellation.ensure_active()?;
        if self.staging == Some(path)
            || path == Path::new(MANIFEST)
            || self.selection.excludes(path)
        {
            return Ok(false);
        }
        let selected = self.selection.includes(path);
        if !selected && !self.selection.could_match_below(path) {
            return Ok(false);
        }

        let absolute = self.root.join(path);
        let metadata = fs::symlink_metadata(&absolute)
            .with_context(|| format!("could not read module path `{}`", path.display()))?;
        let link = is_link_like(&metadata);
        let member = if metadata.is_dir() && !link {
            let mut retained = selected;
            for entry in fs::read_dir(&absolute)
                .with_context(|| format!("could not list module directory `{}`", path.display()))?
            {
                let entry = entry.context("could not read a module directory entry")?;
                retained |= self.visit(&path.join(entry.file_name()))?;
            }
            retained.then_some(Member::Directory)
        } else if selected {
            ensure!(
                metadata.is_file() && !link,
                "selected member `{}` must be an ordinary file or directory without links",
                path.display()
            );
            Some(Member::File(file_mode(&metadata)))
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
