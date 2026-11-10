//! Map one configured asset source to the output path it publishes.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::section::{AssetUrlError, AssetUrlPrefix};
use tola_address::{OutputPath, OutputPathError};

use super::AssetSourceError;

#[derive(Debug, Clone)]
pub(crate) struct AssetOutput {
    /// The logical source coordinate this output is published from.
    pub(crate) logical_source: PathBuf,
    /// Validated path relative to the output root.
    pub(crate) output: tola_address::OutputPath,
    pub(crate) module: Option<super::urls::ModuleAsset>,
    /// Meaning and response media type declared from the logical source.
    declaration: crate::output::semantics::OutputDeclaration,
    /// Identity of the bytes published at `output`, when cache busting asked for one.
    pub(super) identity: Option<Arc<str>>,
}

impl AssetOutput {
    pub(crate) fn declaration(&self) -> &crate::output::semantics::OutputDeclaration {
        &self.declaration
    }

    /// The published bytes' identity, when this output has one.
    pub(crate) fn identity(&self) -> Option<&str> {
        self.identity.as_deref()
    }
}

/// Map an observed asset-tree member without reading the filesystem.
pub(super) fn output_for_configured_tree_member(
    logical_source: &Path,
    relative: &Path,
    url_prefix: &AssetUrlPrefix,
    display_root: &Path,
) -> Result<AssetOutput, AssetOutputError> {
    let member = filesystem_output_path(relative, logical_source, display_root)?;
    let output =
        url_prefix
            .output_for(&member)
            .map_err(|cause| AssetOutputError::ReservedPath {
                member: tola_diagnostic::display_path(logical_source, display_root),
                prefix: url_prefix.to_string(),
                cause,
            })?;
    Ok(AssetOutput {
        logical_source: logical_source.to_path_buf(),
        output,
        declaration: crate::output::semantics::OutputDeclaration::from_filesystem_source(
            logical_source,
        ),
        identity: None,
        module: None,
    })
}

/// Map one configured exact file while preserving its logical source owner.
pub(super) fn output_for_configured_file(
    logical_source: &Path,
    output: &tola_address::OutputPath,
) -> AssetOutput {
    AssetOutput {
        logical_source: logical_source.to_path_buf(),
        output: output.clone(),
        declaration: crate::output::semantics::OutputDeclaration::from_filesystem_source(
            logical_source,
        ),
        identity: None,
        module: None,
    }
}

/// Characters a published member path may not contain; `asset-url()` reads URL syntax.
const DECLARED_URL_SYNTAX: [char; 3] = ['?', '#', '%'];

#[derive(Clone, Copy, Debug)]
pub(super) enum MemberPathViolation {
    NotAPlainName,
    NotUtf8,
    UrlSyntax,
}

/// Why one configured asset cannot be published at the path its declaration names.
#[derive(Debug, thiserror::Error)]
pub(super) enum AssetOutputError {
    /// The member path breaks one of the naming rules.
    MemberPath {
        member: String,
        violation: MemberPathViolation,
    },
    /// The published name is not portable across filesystems.
    PortableName {
        member: String,
        #[source]
        cause: OutputPathError,
    },
    /// The prefix puts the member inside Tola's reserved path.
    ReservedPath {
        member: String,
        prefix: String,
        #[source]
        cause: AssetUrlError,
    },
}

impl fmt::Display for AssetOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MemberPath { member, violation } => match violation {
                MemberPathViolation::NotAPlainName => write!(
                    formatter,
                    "asset `{member}` cannot be published: its path must contain only file and \
                     directory names; rename the file or directory"
                ),
                MemberPathViolation::NotUtf8 => write!(
                    formatter,
                    "asset `{member}` cannot be published because its name is not valid UTF-8; \
                     rename the file or directory"
                ),
                MemberPathViolation::UrlSyntax => write!(
                    formatter,
                    "asset `{member}` cannot be published because its path contains `?`, `#`, \
                     or `%`; rename the file or directory"
                ),
            },
            Self::PortableName { member, .. } => write!(
                formatter,
                "asset `{member}` cannot be published because its name is not portable across \
                 filesystems; rename it"
            ),
            Self::ReservedPath { member, prefix, .. } => write!(
                formatter,
                "`assets.trees` cannot publish `{member}` under `{prefix}` because the output \
                 lands in Tola's reserved `_tola` path; rename the member or its `url-prefix`"
            ),
        }
    }
}

/// Where one published member path splits, or the rule it breaks.
pub(super) fn member_path_segments(relative: &Path) -> Result<Vec<&str>, MemberPathViolation> {
    let mut segments = Vec::new();
    for component in relative.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(MemberPathViolation::NotAPlainName);
        };
        let segment = segment.to_str().ok_or(MemberPathViolation::NotUtf8)?;
        if segment.contains(DECLARED_URL_SYNTAX) {
            return Err(MemberPathViolation::UrlSyntax);
        }
        segments.push(segment);
    }
    Ok(segments)
}

fn filesystem_output_path(
    path: &Path,
    source: &Path,
    display_root: &Path,
) -> Result<OutputPath, AssetOutputError> {
    let member = tola_diagnostic::display_path(source, display_root);
    let segments =
        member_path_segments(path).map_err(|violation| AssetOutputError::MemberPath {
            member: member.clone(),
            violation,
        })?;
    OutputPath::parse(&segments.join("/"))
        .map_err(|cause| AssetOutputError::PortableName { member, cause })
}

/// Refuse outputs that share a portable path or overlap as file and directory.
pub(super) fn check_paths<'output>(
    outputs: impl IntoIterator<Item = &'output AssetOutput>,
    display_root: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
) -> anyhow::Result<()> {
    let mut ordered = outputs
        .into_iter()
        .map(|output| (output, output.output.portable_key()))
        .collect::<Vec<_>>();
    ordered.sort_by(|(_, left), (_, right)| left.cmp(right));
    for pair in ordered.windows(2) {
        cancellation.ensure_active()?;
        let [(output, output_key), (conflict, conflict_key)] = pair else {
            unreachable!("windows(2) always yields two outputs");
        };
        if conflict_key.starts_with(output_key) {
            return Err(AssetSourceError::PublishedPathsConflict {
                left: tola_diagnostic::display_path(&output.logical_source, display_root),
                left_at: output.output.to_string(),
                right: tola_diagnostic::display_path(&conflict.logical_source, display_root),
                right_at: conflict.output.to_string(),
            }
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_prefix_composes_member_paths() {
        let prefix = AssetUrlPrefix::parse("/images/中文/").unwrap();
        let output = output_for_configured_tree_member(
            Path::new("static/a b.svg"),
            Path::new("a b.svg"),
            &prefix,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(output.output.as_str(), "images/中文/a b.svg");
        for member in ["../outside", "bad%20name", "bad#name", "nul.txt"] {
            assert!(
                output_for_configured_tree_member(
                    Path::new(member),
                    Path::new(member),
                    &prefix,
                    Path::new("."),
                )
                .is_err(),
                "{member}"
            );
        }
        assert!(
            output_for_configured_tree_member(
                Path::new("_tola/private"),
                Path::new("_tola/private"),
                &AssetUrlPrefix::parse("/").unwrap(),
                Path::new("."),
            )
            .is_err()
        );
    }

    #[test]
    fn media_type_follows_source() {
        let source = Path::new("assets/app.css");
        let output_path = tola_address::OutputPath::parse("styles/app").unwrap();

        let output = output_for_configured_file(source, &output_path);

        assert_eq!(
            output.declaration().semantics(),
            crate::output::semantics::DeclaredOutputSemantics::Css
        );
        assert_eq!(
            output.declaration().media_type(),
            &crate::output::semantics::ResponseMediaType::CSS
        );
    }
}
