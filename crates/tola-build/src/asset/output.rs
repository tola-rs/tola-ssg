//! Mapping one configured asset source to the output path it publishes, with the
//! identity of the bytes a build rendered for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::section::AssetUrlPrefix;
use anyhow::{Result, anyhow};
use tola_address::OutputPath;

#[derive(Debug, Clone)]
pub(crate) struct AssetOutput {
    /// The logical source coordinate this output is published from; the inventory
    /// retains the physical read evidence separately.
    pub(crate) logical_source: PathBuf,
    /// Validated path relative to the output root.
    pub(crate) output: tola_address::OutputPath,
    /// Meaning and response media type declared from the logical source.
    declaration: crate::output::semantics::OutputDeclaration,
    /// Identity of the bytes published at `output`, when this site asked for
    /// cache busting and a build rendered them. `asset-url()` appends it to the
    /// browser URL as `?h=<identity>`; it never enters a published path.
    identity: Option<Arc<str>>,
}

impl AssetOutput {
    pub(crate) fn declaration(&self) -> &crate::output::semantics::OutputDeclaration {
        &self.declaration
    }

    /// The published bytes' identity, when this output has one.
    pub(crate) fn identity(&self) -> Option<&str> {
        self.identity.as_deref()
    }

    /// The same output, published from bytes this build rendered.
    pub(crate) fn with_identity(mut self, identity: Option<Arc<str>>) -> Self {
        self.identity = identity;
        self
    }
}

/// Map an observed asset-tree member without reading the filesystem.
pub(crate) fn output_for_configured_tree_member(
    logical_source: &Path,
    relative: &Path,
    url_prefix: &AssetUrlPrefix,
    display_root: &Path,
) -> Result<AssetOutput> {
    let member = filesystem_output_path(relative, logical_source, display_root)?;
    Ok(AssetOutput {
        logical_source: logical_source.to_path_buf(),
        output: url_prefix.output_for(&member).map_err(|_| {
            anyhow!(
                "`assets.trees` cannot publish `{}` under `{}` because the output lands in Tola's reserved `_tola` path; rename the member or its `url-prefix`",
                crate::filesystem::display_path(logical_source, display_root),
                url_prefix
            )
        })?,
        declaration: crate::output::semantics::OutputDeclaration::from_filesystem_source(
            logical_source,
        ),
        identity: None,
    })
}

/// Map one configured exact file while preserving its logical source owner.
pub(crate) fn output_for_configured_file(
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
    }
}

/// The characters a published tree member's path may not contain.
///
/// A member's path is also the tail of the site-root URL `asset-url()` resolves, and that lookup
/// reads URL syntax: it decodes percent escapes exactly once and refuses `?` and `#` before
/// decoding. A member whose name contains one of these could only be named by a URL spelling its
/// name does not have, so Tola refuses the member rather than guess which reading it meant.
pub(super) const DECLARED_URL_SYNTAX: [char; 3] = ['?', '#', '%'];

fn filesystem_output_path(path: &Path, source: &Path, display_root: &Path) -> Result<OutputPath> {
    let member = crate::filesystem::display_path(source, display_root);
    let mut segments = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(anyhow!(
                "asset `{member}` must have a path containing only file and directory names; rename it"
            ));
        };
        let segment = segment.to_str().ok_or_else(|| {
            anyhow!("asset `{member}` cannot be published because its name is not valid UTF-8; rename it")
        })?;
        if segment.contains(DECLARED_URL_SYNTAX) {
            return Err(anyhow!(
                "asset `{member}` cannot be published because its path contains `?`, `#`, or `%`; rename it"
            ));
        }
        segments.push(segment);
    }
    OutputPath::parse(&segments.join("/"))
        .map_err(|_| anyhow!("asset `{member}` cannot be published because its name is not portable across filesystems; rename it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn tree_output_ignores_the_deployment_mount() {
        let dir = TempDir::new().unwrap();
        let images = dir.path().join("assets/images");
        fs::create_dir_all(&images).unwrap();
        let source = images.join("logo.png");
        fs::write(&source, "image").unwrap();

        let mut config = crate::config::tests::load_test_config(
            dir.path(),
            "[site]\norigin = \"https://example.test\"\nbase-path = \"/docs/blog/\"",
        );
        config.build.publish_dir = dir.path().join("public");
        let output = output_for_configured_tree_member(
            &source,
            Path::new("logo.png"),
            &AssetUrlPrefix::parse("/images").unwrap(),
            dir.path(),
        )
        .unwrap();

        assert_eq!(output.logical_source, source);
        assert_eq!(output.output.as_str(), "images/logo.png");
        let url = tola_address::asset_url_from_output(&output.output);
        assert_eq!(url, "/images/logo.png");
        assert_eq!(
            config.url_mount().browser_path(&url),
            "/docs/blog/images/logo.png"
        );
    }

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
    fn file_output_uses_the_declared_name() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("assets/favicon.ico");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "icon").unwrap();

        let mut config = crate::config::tests::load_test_config(
            dir.path(),
            "[site]\norigin = \"https://example.test\"\nbase-path = \"/docs/blog/\"",
        );
        config.build.publish_dir = dir.path().join("public");
        let url = crate::config::section::AssetUrl::parse("/favicon.ico").unwrap();
        let output = output_for_configured_file(&source, url.output_path());

        assert_eq!(output.output.as_str(), "favicon.ico");
        let url = tola_address::asset_url_from_output(&output.output);
        assert_eq!(url, "/favicon.ico");
        assert_eq!(
            config.url_mount().browser_path(&url),
            "/docs/blog/favicon.ico"
        );
    }

    #[test]
    fn file_media_type_follows_the_source() {
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
