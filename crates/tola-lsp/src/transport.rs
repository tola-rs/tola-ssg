//! Bounded Content-Length framing around the standard LSP message model.

use std::io::{BufRead, Read, Write};

use anyhow::{Context, Result, bail};
use lsp_server::{ErrorCode, Message, RequestId, Response, ResponseError};
use serde::Serialize;
use serde_json::Value;

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const JSON_RPC_VERSION: &str = "2.0";

const UNREADABLE_MESSAGE: &str =
    "Tola could not read the editor's message; restart the language server";
const OVERSIZED_MESSAGE: &str =
    "the editor's message is larger than Tola accepts; restart the language server";
const TRUNCATED_MESSAGE: &str =
    "the editor closed the connection partway through a message; restart the language server";

pub(super) fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut header = String::new();
    let mut header_bytes = 0;
    loop {
        header.clear();
        let read = (&mut *reader)
            .take((MAX_HEADER_BYTES - header_bytes + 1) as u64)
            .read_line(&mut header)?;
        header_bytes += read;
        if header_bytes > MAX_HEADER_BYTES {
            bail!("{OVERSIZED_MESSAGE}");
        }
        if read == 0 {
            if header_bytes == 0 {
                return Ok(None);
            }
            bail!("{TRUNCATED_MESSAGE}");
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        let (name, value) = header.split_once(':').context(UNREADABLE_MESSAGE)?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                bail!("{UNREADABLE_MESSAGE}");
            }
            length = Some(value.trim().parse::<usize>().context(UNREADABLE_MESSAGE)?);
        }
    }
    let length = length.context(UNREADABLE_MESSAGE)?;
    if length > MAX_BODY_BYTES {
        bail!("{OVERSIZED_MESSAGE}");
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .with_context(|| TRUNCATED_MESSAGE)?;
    Ok(Some(bytes))
}

#[derive(Debug)]
pub(super) struct InvalidMessage {
    pub id: Option<RequestId>,
    pub error: Box<ResponseError>,
}

impl InvalidMessage {
    fn new(id: Option<RequestId>, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            id,
            error: Box::new(ResponseError {
                code: code as i32,
                message: message.into(),
                data: None,
            }),
        }
    }
}

const UNREADABLE_REQUEST: &str = "the Tola language server could not read this request";

pub(super) fn decode_message(bytes: &[u8]) -> std::result::Result<Message, InvalidMessage> {
    let envelope: Value = serde_json::from_slice(bytes)
        .map_err(|_| InvalidMessage::new(None, ErrorCode::ParseError, UNREADABLE_REQUEST))?;
    let id = envelope
        .get("id")
        .map(|id| serde_json::from_value::<RequestId>(id.clone()))
        .transpose()
        .map_err(|_| InvalidMessage::new(None, ErrorCode::InvalidRequest, UNREADABLE_REQUEST))?;
    // `Message`'s untagged decoder ignores unknown fields, so the version and ID
    // are validated here; an invalid request must not become a notification.
    if !envelope.is_object()
        || envelope["jsonrpc"] != JSON_RPC_VERSION
        || envelope
            .get("method")
            .is_some_and(|method| !method.is_string())
        || (envelope.get("result").is_some() && envelope.get("error").is_some())
    {
        return Err(InvalidMessage::new(
            id,
            ErrorCode::InvalidRequest,
            UNREADABLE_REQUEST,
        ));
    }
    serde_json::from_value(envelope)
        .map_err(|_| InvalidMessage::new(id, ErrorCode::InvalidRequest, UNREADABLE_REQUEST))
}

pub(super) fn write_invalid(writer: &mut impl Write, invalid: InvalidMessage) -> Result<()> {
    if let Some(id) = invalid.id {
        let message: Message = Response {
            id,
            response_result: Err(*invalid.error),
        }
        .into();
        message.write(writer)?;
        return Ok(());
    }
    // JSON-RPC needs `id: null` here; lsp-server's `Response` cannot hold it.
    #[derive(Serialize)]
    struct UnidentifiedError {
        jsonrpc: &'static str,
        id: (),
        error: ResponseError,
    }
    let bytes = serde_json::to_vec(&UnidentifiedError {
        jsonrpc: JSON_RPC_VERSION,
        id: (),
        error: *invalid.error,
    })?;
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::{Notification, Request};
    use serde_json::json;

    #[test]
    fn conflicting_envelopes_are_refused() {
        let envelope = json!({"jsonrpc":"2.0","id":1,"result":null,"error":{"code":-32603,"message":"failed"}});
        let invalid = decode_message(&serde_json::to_vec(&envelope).unwrap()).unwrap_err();
        assert_eq!(invalid.error.code, ErrorCode::InvalidRequest as i32);
    }

    #[test]
    fn oversized_frames_are_refused() {
        let mut oversized = std::io::Cursor::new(vec![b'x'; MAX_HEADER_BYTES + 1]);
        assert!(read_frame(&mut oversized).is_err());
        assert_eq!(oversized.position(), (MAX_HEADER_BYTES + 1) as u64);
        let header = format!("Content-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        assert!(read_frame(&mut std::io::Cursor::new(header)).is_err());
        let many_headers = "X: a\r\n".repeat(MAX_HEADER_BYTES / 6 + 1);
        assert!(read_frame(&mut std::io::Cursor::new(many_headers)).is_err());
    }

    #[test]
    fn frames_split_on_byte_lengths() {
        let mut bytes = Vec::new();
        let first = Message::Response(Response::new_ok(1.into(), "你好"));
        let second = Message::Notification(Notification::new("exit".into(), ()));
        first.write(&mut bytes).unwrap();
        second.write(&mut bytes).unwrap();
        let mut reader = std::io::Cursor::new(bytes);
        let first = decode_message(&read_frame(&mut reader).unwrap().unwrap()).unwrap();
        let Message::Response(first) = first else {
            panic!("expected response")
        };
        assert_eq!(first.id, RequestId::from(1));
        assert_eq!(first.response_result.unwrap(), json!("你好"));
        let second = decode_message(&read_frame(&mut reader).unwrap().unwrap()).unwrap();
        assert!(
            matches!(second, Message::Notification(notification) if notification.method == "exit")
        );
        assert!(read_frame(&mut reader).unwrap().is_none());
    }

    #[test]
    fn content_length_defects_are_refused() {
        for frame in [
            "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "Content-Length: 3\r\n\r\n{}",
            "Content-Length: -1\r\n\r\n",
        ] {
            assert!(read_frame(&mut std::io::Cursor::new(frame)).is_err());
        }
    }

    #[test]
    fn invalid_ids_never_become_notifications() {
        for id in [json!(true), json!(i64::MAX)] {
            let bytes =
                serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":"shutdown"})).unwrap();
            let invalid = decode_message(&bytes).unwrap_err();
            assert_eq!(invalid.error.code, ErrorCode::InvalidRequest as i32);
            assert!(invalid.id.is_none());
        }
    }

    /// A malformed envelope still has the request id it parsed, so the client can
    /// correlate the error answer.
    #[test]
    fn malformed_requests_keep_their_id() {
        let invalid =
            decode_message(br#"{"jsonrpc":"1.0","id":"known","method":"shutdown"}"#).unwrap_err();
        assert_eq!(invalid.id, Some(RequestId::from("known".to_owned())));
    }

    /// A body the server cannot parse is answered without an id, which JSON-RPC requires.
    #[test]
    fn parse_errors_reply_without_id() {
        let invalid = decode_message(b"{").unwrap_err();
        assert_eq!(invalid.error.code, ErrorCode::ParseError as i32);
        let mut bytes = Vec::new();
        write_invalid(&mut bytes, invalid).unwrap();
        let reply: Value = serde_json::from_slice(
            &read_frame(&mut std::io::Cursor::new(bytes))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reply["jsonrpc"], "2.0");
        assert!(reply["id"].is_null());
        assert_eq!(reply["error"]["code"], ErrorCode::ParseError as i32);
    }

    /// The validation around unreadable envelopes never rejects valid traffic.
    #[test]
    fn well_formed_requests_decode() {
        let request = Request::new(2.into(), "shutdown".into(), ());
        let mut bytes = Vec::new();
        let request: Message = request.into();
        request.write(&mut bytes).unwrap();
        assert!(matches!(
            decode_message(
                &read_frame(&mut std::io::Cursor::new(bytes))
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            Message::Request(_)
        ));
    }
}
