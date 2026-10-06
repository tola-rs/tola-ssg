//! Counts and page availability of complete output graphs.

use serde::Serialize;

use super::graph::{OutputFile, OutputKind};

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
