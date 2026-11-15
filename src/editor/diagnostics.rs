//! File identities for local editor consumers of command diagnostics.

use std::path::Path;

use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};
use tola_diagnostic::{Diagnostic, DiagnosticSource, Location};
use tola_input::Sha256Digest;

#[derive(Serialize)]
struct File<'a> {
    path: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<Sha256Digest>,
}

impl<'a> File<'a> {
    fn located(
        location: Option<&'a Location>,
        source: Option<&'a DiagnosticSource>,
    ) -> Option<Self> {
        let location = location?;
        match source {
            Some(source) => Some(Self {
                path: source.editor_path()?,
                sha256: source.sha256(),
            }),
            None => Some(Self {
                path: location.file.as_deref()?,
                sha256: None,
            }),
        }
    }
}

#[derive(Serialize)]
struct Files<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    primary: Option<File<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    help: Vec<Option<File<'a>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    related: Vec<Option<File<'a>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    trace: Vec<Option<File<'a>>>,
}

impl<'a> Files<'a> {
    fn of(diagnostic: &'a Diagnostic) -> Option<Self> {
        let primary = File::located(diagnostic.location.as_ref(), diagnostic.source.as_ref());
        let help = files(
            diagnostic
                .help
                .iter()
                .map(|item| File::located(item.location.as_ref(), None)),
        );
        let related = files(
            diagnostic
                .related
                .iter()
                .map(|item| File::located(item.location.as_ref(), item.source.as_ref())),
        );
        let trace = files(
            diagnostic
                .trace
                .iter()
                .map(|item| File::located(item.location.as_ref(), None)),
        );
        (primary.is_some() || !help.is_empty() || !related.is_empty() || !trace.is_empty())
            .then_some(Self {
                primary,
                help,
                related,
                trace,
            })
    }
}

fn files<'a>(locations: impl Iterator<Item = Option<File<'a>>>) -> Vec<Option<File<'a>>> {
    let mut locations: Vec<_> = locations.collect();
    while locations.last().is_some_and(Option::is_none) {
        locations.pop();
    }
    locations
}

#[derive(Serialize)]
struct Report<'a> {
    #[serde(flatten)]
    diagnostic: &'a Diagnostic,
    #[serde(skip_serializing_if = "Option::is_none")]
    editor: Option<Files<'a>>,
}

pub(crate) fn serialize<S: Serializer>(
    diagnostics: &[Diagnostic],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut sequence = serializer.serialize_seq(Some(diagnostics.len()))?;
    for diagnostic in diagnostics {
        sequence.serialize_element(&Report {
            diagnostic,
            editor: Files::of(diagnostic),
        })?;
    }
    sequence.end()
}
