//! The compilable query copy: one unfinished construct blanked, every byte offset kept.
//!
//! The official compiler refuses a source an author is still writing, so a query that needs the
//! world compiles a copy in which the construct the cursor stands in is blanked or bound. The
//! author's own snapshot stays untouched; only the override set has the copy.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, SyntaxKind, VirtualRoot};

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
    pin_source(source, text.into(), config, overrides)?;
    Ok(())
}
pub(super) fn blank_query_syntax(
    source: &Source,
    range: std::ops::Range<usize>,
    config: &ResolvedSiteConfig,
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
    pin_source(source, text.into(), config, overrides)?;
    Ok(())
}
pub(crate) fn pin_source(
    source: &Source,
    text: Arc<str>,
    config: &ResolvedSiteConfig,
    overrides: &mut Vec<(PathBuf, Arc<str>)>,
) -> Result<()> {
    if source.id().root() != &VirtualRoot::Project {
        bail!("embedded package documents are immutable");
    }
    let mut found = false;
    for (path, previous) in overrides.iter_mut() {
        if crate::identity::path_id(path, config.get_root()) == Some(source.id()) {
            if previous.as_ref() != source.text() {
                bail!("conflicting unsaved text for the same source");
            }
            *previous = Arc::clone(&text);
            found = true;
        }
    }
    if !found {
        overrides.push((
            config
                .get_root()
                .join(source.id().vpath().get_without_slash()),
            text,
        ));
    }
    Ok(())
}
