//! The events one connection's threads publish to its serving loop.

use std::time::Duration;

use crossbeam_channel::{SendTimeoutError, Sender};
use lsp_server::Message;
use tola_build::cancellation::BuildCancellation;

use crate::compiler::SourceCompilation;
use crate::transport::InvalidMessage;

/// How long a thread waits on the event channel before it observes cancellation again.
pub(super) const WAIT_INTERVAL: Duration = Duration::from_millis(25);

pub(super) enum Event {
    Message(std::result::Result<Message, InvalidMessage>),
    Compiled(SourceCompilation),
    Analyzed(SourceCompilation),
    InputClosed,
    InputFailed(anyhow::Error),
}

pub(super) fn send_event(
    events: &Sender<Event>,
    mut event: Event,
    cancellation: &BuildCancellation,
) -> bool {
    loop {
        if cancellation.is_cancelled() {
            return false;
        }
        match events.send_timeout(event, WAIT_INTERVAL) {
            Ok(()) => return true,
            Err(SendTimeoutError::Disconnected(_)) => return false,
            Err(SendTimeoutError::Timeout(returned)) => event = returned,
        }
    }
}
