//! What one development build round reports to the terminal.

use chrono::{DateTime, Local};
use tola_diagnostic::Diagnostic;

/// One build attempt's number and the revision it published, when it did.
#[derive(Clone, Copy)]
pub(crate) struct RoundIdentity<'a> {
    pub(crate) number: u64,
    pub(crate) revision: Option<&'a str>,
}

/// One finished build attempt as the development view reads it.
pub(crate) struct Round {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) transcript: String,
    pub(crate) observed_at: DateTime<Local>,
}

/// The stream index a hook event names; both streams share one execution's parsers.
pub(crate) fn stream_index(stream: &str) -> usize {
    usize::from(stream == "stderr")
}

/// The hook run one line of output belongs to.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HookRun {
    pub(crate) scope: u64,
    pub(crate) name: String,
}

/// How one build attempt ended.
#[derive(Clone, Copy)]
pub(crate) enum Completion<'a> {
    Published(&'a str),
    Failed(&'a str),
}

/// The identity and outcome of one finished build round.
#[derive(Clone, Copy)]
pub(crate) struct CompletedRound<'a> {
    pub(crate) identity: Option<RoundIdentity<'a>>,
    pub(crate) completion: Completion<'a>,
}

impl<'a> CompletedRound<'a> {
    pub(crate) fn published(number: u64, revision: &'a str, summary: &'a str) -> Self {
        Self {
            identity: Some(RoundIdentity {
                number,
                revision: Some(revision),
            }),
            completion: Completion::Published(summary),
        }
    }

    pub(crate) fn initial(summary: &'a str) -> Self {
        Self {
            identity: None,
            completion: Completion::Published(summary),
        }
    }

    pub(crate) fn failed(number: Option<u64>, message: &'a str) -> Self {
        Self {
            identity: number.map(|number| RoundIdentity {
                number,
                revision: None,
            }),
            completion: Completion::Failed(message),
        }
    }

    pub(crate) fn identity(self) -> Option<RoundIdentity<'a>> {
        self.identity
    }
}
