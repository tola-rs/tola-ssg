//! Stable output identities and producers.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use tola_address::OutputPath;

/// Exclusive ownership of every output beneath one logical directory root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRootOwnership {
    root: OutputPath,
    owner: OutputOwner,
}

impl OutputRootOwnership {
    pub(crate) fn new(root: OutputPath, owner: OutputOwner) -> Self {
        Self { root, owner }
    }
    pub fn root(&self) -> &OutputPath {
        &self.root
    }
    pub fn owner(&self) -> &OutputOwner {
        &self.owner
    }
}

/// The producer that owns an output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputOwner {
    /// A document or raw asset emitted by the root Typst Bundle.
    Bundle { producer: Arc<str> },
    /// A configured asset owned by one logical source coordinate.
    ConfiguredAsset { source: PathBuf },
    /// A Tola-generated output.
    System { producer: Arc<str> },
    /// Files declared by one configured output command.
    Command { index: usize, name: Arc<str> },
    /// A file supplied directly by a native host producer.
    Generated { producer: Arc<str> },
}

impl OutputOwner {
    pub(crate) fn bundle(producer: impl Into<Arc<str>>) -> Self {
        Self::Bundle {
            producer: producer.into(),
        }
    }

    pub(crate) fn configured_asset(source: impl Into<PathBuf>) -> Self {
        Self::ConfiguredAsset {
            source: source.into(),
        }
    }

    pub(crate) fn system(producer: impl Into<Arc<str>>) -> Self {
        Self::System {
            producer: producer.into(),
        }
    }

    pub(crate) fn command(index: usize, name: impl Into<Arc<str>>) -> Self {
        Self::Command {
            index,
            name: name.into(),
        }
    }

    pub(crate) fn is_system(&self) -> bool {
        matches!(self, Self::System { .. })
    }

    pub fn configured_source(&self) -> Option<&std::path::Path> {
        match self {
            Self::ConfiguredAsset { source } => Some(source),
            Self::Bundle { .. }
            | Self::System { .. }
            | Self::Command { .. }
            | Self::Generated { .. } => None,
        }
    }
}

impl fmt::Display for OutputOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bundle { .. } => f.write_str("the root Bundle"),
            // The stored coordinate is an absolute filesystem path; name its last
            // component so a rendered conflict never prints a host path.
            Self::ConfiguredAsset { source } => match source.file_name() {
                Some(name) => write!(f, "the configured asset `{}`", name.to_string_lossy()),
                None => f.write_str("a configured asset"),
            },
            Self::System { .. } => f.write_str("Tola"),
            // The owner stores the label the console shows, which is the configured name or the
            // command the author left unnamed.
            Self::Command { name, .. } => write!(
                f,
                "{}",
                crate::config::section::build::hooks::hook_identity(
                    crate::config::section::build::hooks::HookStage::GenerateOutputs,
                    name
                )
            ),
            Self::Generated { .. } => f.write_str("the embedding program"),
        }
    }
}
