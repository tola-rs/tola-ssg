//! The compilable query copy: one unfinished construct blanked, every byte offset kept.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use thiserror::Error;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, SyntaxKind, VirtualRoot};

use crate::packages::PackageAccess;

/// Why one source's query copy cannot be pinned.
#[derive(Debug, Error)]
pub(crate) enum RepairError {
    #[error("this source has no editable backing file")]
    NoEditablePath,
    #[error("conflicting unsaved text for the same source")]
    ConflictingText,
}

/// The star of the import the cursor is on, when the source writes one.
pub(super) fn import_star(source: &Source, cursor: usize) -> Option<LinkedNode<'_>> {
    let node = LinkedNode::new(source.root()).leaf_at(cursor, Side::After)?;
    (node.kind() == SyntaxKind::Star && node.parent_kind() == Some(SyntaxKind::ModuleImport))
        .then_some(node)
}
pub(super) fn blank_query_name(
    source: &Source,
    range: std::ops::Range<usize>,
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    overrides: &mut Vec<(PathBuf, Arc<str>)>,
) -> Result<()> {
    // A lone hash is not a name yet: dropping it leaves the text the author typed. An unfinished
    // identifier becomes a function value, valid both as a value and as a callee.
    let author = &source.text()[range.clone()];
    let replacement = if author.starts_with('#') {
        ""
    } else if range.len() >= "repr".len() {
        "repr"
    } else {
        "0"
    };
    let mut text = source.text().to_owned();
    text.replace_range(
        range.clone(),
        &format!("{replacement:<width$}", width = range.len()),
    );
    pin_source(source, text.into(), config, package_access, overrides)?;
    Ok(())
}
pub(super) fn blank_query_syntax(
    source: &Source,
    range: std::ops::Range<usize>,
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    overrides: &mut Vec<(PathBuf, Arc<str>)>,
) -> Result<()> {
    // Keep byte offsets and newline sequences intact in the compiled query tree.
    let mut text = String::with_capacity(source.text().len());
    text.push_str(&source.text()[..range.start]);
    text.extend(source.text()[range.clone()].bytes().map(|byte| {
        if matches!(byte, b'\n' | b'\r') {
            char::from(byte)
        } else {
            ' '
        }
    }));
    text.push_str(&source.text()[range.end..]);
    pin_source(source, text.into(), config, package_access, overrides)?;
    Ok(())
}
pub(crate) fn pin_source(
    source: &Source,
    text: Arc<str>,
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    overrides: &mut Vec<(PathBuf, Arc<str>)>,
) -> std::result::Result<(), RepairError> {
    let Some(target) = editable_source_path(source, config, package_access) else {
        return Err(RepairError::NoEditablePath);
    };
    let target = tola_build::filesystem::normalize_existing_prefix(&target);
    let mut found = false;
    for (path, previous) in overrides.iter_mut() {
        if tola_build::filesystem::normalize_existing_prefix(path) == target {
            if previous.as_ref() != source.text() {
                return Err(RepairError::ConflictingText);
            }
            *previous = Arc::clone(&text);
            found = true;
        }
    }
    if !found {
        overrides.push((target, text));
    }
    Ok(())
}

pub(super) fn editable_source_path(
    source: &Source,
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
) -> Option<PathBuf> {
    let path = match source.id().root() {
        VirtualRoot::Project => config
            .get_root()
            .join(source.id().vpath().get_without_slash()),
        VirtualRoot::Package(_) => {
            if crate::identity::embedded_source(source.id()).is_some() {
                return None;
            }
            crate::identity::package_file_path_with_packages(
                source.id(),
                &package_access.locations,
                &package_access.boundary,
                &package_access.local_packages,
            )?
        }
    };
    package_access.boundary.check(&path).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::tests::QuerySession;
    use tola_typst::typst::syntax::{RootedPath, VirtualPath};

    #[test]
    fn embedded_sources_are_not_editable() {
        let site = QuerySession::new();
        let package_access = PackageAccess::prepared(&site.config, site.session.resources(), &[])
            .expect("package access");
        let id = RootedPath::new(
            VirtualRoot::Package("@tola/source:0.0.0".parse().expect("a package spec")),
            VirtualPath::new("lib.typ").expect("a package path"),
        )
        .intern();
        let text = crate::identity::embedded_source(id).expect("builtin source");
        let source = Source::new(id, text.into_owned());
        assert!(editable_source_path(&source, &site.config, &package_access).is_none());
        assert!(
            pin_source(
                &source,
                Arc::from("replacement"),
                &site.config,
                &package_access,
                &mut Vec::new()
            )
            .is_err()
        );
    }
}
