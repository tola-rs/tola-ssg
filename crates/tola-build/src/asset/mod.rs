//! Configured assets and their published paths and URLs.

mod build;
mod declarations;
pub(crate) mod minify;
mod output;
mod source;
mod urls;

pub(crate) use build::{ConfiguredAssets, build_configured_assets};
pub(crate) use minify::AssetMinifyWarning;
pub use urls::{AssetOrigin, AssetUrls, ModuleAsset, ModuleAssetInput, PublishedAsset};

/// Why one configured asset source cannot be resolved or published.
#[derive(Debug, thiserror::Error)]
enum AssetSourceError {
    /// A declared source does not exist.
    #[error(
        "asset source `{path}` declared in `{key}` does not exist; create the file or fix the \
         declaration"
    )]
    SourceMissing { path: String, key: &'static str },
    /// A declared source cannot be read.
    #[error(
        "cannot read asset source `{path}` declared in `{key}`: {reason}; check that the file is \
         readable"
    )]
    SourceUnreadable {
        path: String,
        key: &'static str,
        reason: &'static str,
    },
    /// A declared source has the wrong kind for its declaration.
    #[error("`{key}` source `{path}` is not a {expected}; fix the declaration")]
    SourceKind {
        key: &'static str,
        path: String,
        expected: &'static str,
    },
    /// A module asset source points outside its input root.
    #[error("asset source `{path}` escapes the {owner} root")]
    SourceEscapes { path: String, owner: &'static str },
    /// A declared source overlaps a directory Tola owns.
    #[error("asset source `{path}` overlaps the {owner}; choose a source outside it")]
    OverlapsProtected { path: String, owner: &'static str },
    /// Two nested tree sources publish overlapping members.
    #[error(
        "asset source `{left}` declared in `{left_key}` overlaps `{right}` declared in \
         `{right_key}`; remove the nested source or choose separate sources"
    )]
    TreeSourcesOverlap {
        left: String,
        left_key: &'static str,
        right: String,
        right_key: &'static str,
    },
    /// Two declarations publish one output path.
    #[error(
        "asset output `{left}` declared in `{left_key}` overlaps output `{right}` declared in \
         `{right_key}`; give each published path one owner"
    )]
    OutputsOverlap {
        left: String,
        left_key: &'static str,
        right: String,
        right_key: &'static str,
    },
    /// Two published paths overlap.
    #[error(
        "asset `{left}` at `{left_at}` conflicts with asset `{right}` at `{right_at}`; rename one \
         so the paths do not overlap"
    )]
    PublishedPathsConflict {
        left: String,
        left_at: String,
        right: String,
        right_at: String,
    },
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::urls::AssetUrls;
    use crate::config::section::{AssetFileDeclaration, AssetUrl};

    pub(crate) fn published_url(urls: &AssetUrls, declared_url: &str) -> Option<String> {
        urls.to_typst_dict()
            .get(declared_url)
            .ok()
            .and_then(|value| value.clone().cast::<typst::foundations::Str>().ok())
            .map(|value| value.as_str().to_owned())
    }

    pub(crate) fn file_declaration(
        source: impl Into<PathBuf>,
        url: impl AsRef<str>,
    ) -> AssetFileDeclaration {
        AssetFileDeclaration::new(source, AssetUrl::parse(url.as_ref()).unwrap())
    }
}
