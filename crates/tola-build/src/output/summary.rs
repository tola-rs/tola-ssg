//! Counts and page availability of complete output graphs.

use serde::Serialize;

use std::collections::BTreeMap;
use std::sync::Arc;

use super::graph::{OutputFile, OutputKind};
use super::owner::OutputOwner;

/// Whether a complete site revision contains at least one HTML document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PageAvailability {
    Empty,
    Present,
}

impl PageAvailability {
    pub fn from_outputs(outputs: &[OutputFile]) -> Self {
        if outputs
            .iter()
            .any(|output| output.kind() == OutputKind::HtmlDocument)
        {
            Self::Present
        } else {
            Self::Empty
        }
    }
}

/// Counts of files by their declared document or asset role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputCounts {
    pub pages: usize,
    pub documents: usize,
    pub assets: usize,
}

impl OutputCounts {
    /// Count outputs by kind, excluding the outputs Tola supplies itself.
    pub fn from_outputs(outputs: &[OutputFile]) -> Self {
        let mut counts = Self::default();
        for output in outputs {
            if output.owner().is_system() {
                continue;
            }
            counts.add_kind(output.kind());
        }
        counts
    }

    pub(crate) fn add_kind(&mut self, kind: OutputKind) {
        match kind {
            OutputKind::HtmlDocument => self.pages += 1,
            OutputKind::PdfDocument | OutputKind::PngDocument | OutputKind::SvgDocument => {
                self.documents += 1
            }
            OutputKind::Asset => self.assets += 1,
        }
    }
}

/// The producer a changed output is attributed to in a rebuild summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputProducer {
    /// A Bundle HTML page.
    Page,
    /// A Bundle PDF, PNG, or SVG document.
    Document,
    /// A non-document Bundle asset.
    BundleAsset,
    /// A `[assets]` source.
    Static,
    /// One `generate-outputs` hook.
    Hook(Arc<str>),
    /// A native host producer.
    Generated,
    /// An output Tola supplies itself; it never appears in a rebuild summary.
    System,
}

impl OutputProducer {
    /// The short label a rebuild diagnostic uses for this producer.
    pub fn label(&self) -> &str {
        match self {
            Self::Page => "page",
            Self::Document => "document",
            Self::BundleAsset => "asset",
            Self::Static => "static",
            Self::Hook(name) => name,
            Self::Generated => "generated",
            Self::System => "system",
        }
    }

    pub(crate) fn from_output(output: &OutputFile) -> Self {
        match output.owner() {
            OutputOwner::Bundle { .. } => match output.kind() {
                OutputKind::HtmlDocument => Self::Page,
                OutputKind::PdfDocument | OutputKind::PngDocument | OutputKind::SvgDocument => {
                    Self::Document
                }
                OutputKind::Asset => Self::BundleAsset,
            },
            OutputOwner::ConfiguredAsset { .. } => Self::Static,
            OutputOwner::Command { name, .. } => Self::Hook(Arc::clone(name)),
            OutputOwner::Generated { .. } => Self::Generated,
            OutputOwner::System { .. } => Self::System,
        }
    }
}

/// Changed outputs grouped by the producer that owns them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedOutputGroups {
    pub pages: usize,
    pub documents: usize,
    pub assets: usize,
    pub static_files: usize,
    pub generated_files: usize,
    pub hooks: BTreeMap<Arc<str>, usize>,
}

impl ChangedOutputGroups {
    pub(crate) fn add(&mut self, producer: &OutputProducer) {
        match producer {
            OutputProducer::Page => self.pages += 1,
            OutputProducer::Document => self.documents += 1,
            OutputProducer::BundleAsset => self.assets += 1,
            OutputProducer::Static => self.static_files += 1,
            OutputProducer::Generated => self.generated_files += 1,
            OutputProducer::Hook(name) => {
                *self.hooks.entry(Arc::clone(name)).or_default() += 1;
            }
            OutputProducer::System => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pages == 0
            && self.documents == 0
            && self.assets == 0
            && self.static_files == 0
            && self.generated_files == 0
            && self.hooks.is_empty()
    }
}
