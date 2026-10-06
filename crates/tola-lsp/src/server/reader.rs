//! The detached thread that turns client input into events.

use std::io::{BufReader, Read};

use crossbeam_channel::Sender;
use lsp_server::Message;
use lsp_types::notification::{self, Notification as LspNotification};
use tola_build::cancellation::BuildCancellation;

use super::events::{Event, send_event};
use crate::transport::{decode_message, read_frame};

pub(super) fn read_messages(
    reader: impl Read,
    events: &Sender<Event>,
    cancellation: &BuildCancellation,
) {
    let mut reader = BufReader::new(reader);
    loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                send_event(events, Event::InputClosed, cancellation);
                return;
            }
            Err(error) => {
                send_event(events, Event::InputFailed(error), cancellation);
                return;
            }
        };
        let message = decode_message(&frame);
        let exiting = matches!(&message, Ok(Message::Notification(message))
            if message.method == notification::Exit::METHOD);
        if !send_event(events, Event::Message(message), cancellation) || exiting {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::serve;
    use tola_build::BuildResources;

    #[test]
    fn failed_input_preserves_caller_scope() {
        struct FailedInput;
        impl Read for FailedInput {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("input failure");
            }
        }
        let cancellation = BuildCancellation::new();
        assert!(
            serve(
                FailedInput,
                Vec::new(),
                BuildResources::default(),
                cancellation.clone(),
                |_, _| unreachable!(),
                |_| {},
                None,
                &[],
            )
            .is_err()
        );
        assert!(!cancellation.is_cancelled());
    }
}
