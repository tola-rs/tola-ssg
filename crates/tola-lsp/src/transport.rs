//! Bounded Content-Length framing around the standard LSP message model.

use std::io::{BufRead, Read, Write};

use anyhow::Result;
use lsp_server::{ErrorCode, Message, RequestId, Response, ResponseError};
use serde::Serialize;
use serde_json::Value;

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const JSON_RPC_VERSION: &str = "2.0";

/// Why one frame could not be read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Tola could not read the editor's message; restart the language server")]
    Unreadable,
    #[error("the editor's message is larger than Tola accepts; restart the language server")]
    Oversized,
    #[error(
        "the editor closed the connection partway through a message; restart the language server"
    )]
    Truncated,
}

pub(super) fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, ReadError> {
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
            return Err(ReadError::Oversized);
        }
        if read == 0 {
            if header_bytes == 0 {
                return Ok(None);
            }
            return Err(ReadError::Truncated);
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        let (name, value) = header.split_once(':').ok_or(ReadError::Unreadable)?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(ReadError::Unreadable);
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| ReadError::Unreadable)?,
            );
        }
    }
    let length = length.ok_or(ReadError::Unreadable)?;
    if length > MAX_BODY_BYTES {
        return Err(ReadError::Oversized);
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| ReadError::Truncated)?;
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
    use lsp_server::Notification;
    use serde_json::json;

    /// Every malformed envelope answers with the code JSON-RPC gives it and the id it could
    /// read: an invalid id must not turn a request into a notification, a conflicting envelope
    /// must not be mistaken for a response, and a body that does not parse is answered without
    /// an id.
    #[test]
    fn malformed_envelopes_answer_their_codes() {
        for (body, code, id) in [
            (
                br#"{"jsonrpc":"2.0","id":1,"result":null,"error":{"code":-32603,"message":"failed"}}"#
                    .as_slice(),
                ErrorCode::InvalidRequest,
                Some(RequestId::from(1)),
            ),
            (
                br#"{"jsonrpc":"2.0","id":true,"method":"shutdown"}"#.as_slice(),
                ErrorCode::InvalidRequest,
                None,
            ),
            (
                br#"{"jsonrpc":"2.0","id":9223372036854775807,"method":"shutdown"}"#.as_slice(),
                ErrorCode::InvalidRequest,
                None,
            ),
            (
                br#"{"jsonrpc":"1.0","id":"known","method":"shutdown"}"#.as_slice(),
                ErrorCode::InvalidRequest,
                Some(RequestId::from("known".to_owned())),
            ),
            (b"{".as_slice(), ErrorCode::ParseError, None),
        ] {
            let invalid = decode_message(body).expect_err("a malformed envelope");
            assert_eq!(invalid.error.code, code as i32, "{body:?}");
            assert_eq!(invalid.id, id, "{body:?}");
        }
        let invalid = decode_message(b"{").expect_err("an unparseable body");
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

    /// A frame the server cannot read is refused with the reason it is refused, rather than read
    /// as traffic; an oversized header consumes its bound, not the rest of the stream.
    #[test]
    fn frame_defects_are_refused() {
        let mut many_headers = String::new();
        for _ in 0..MAX_HEADER_BYTES / 6 + 1 {
            many_headers.push_str("X: a\r\n");
        }
        let long_body = format!("Content-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        for (bytes, expected, consumed) in [
            (
                vec![b'x'; MAX_HEADER_BYTES + 1],
                ReadError::Oversized,
                Some((MAX_HEADER_BYTES + 1) as u64),
            ),
            (long_body.into_bytes(), ReadError::Oversized, None),
            (many_headers.into_bytes(), ReadError::Oversized, None),
            (
                b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
                ReadError::Unreadable,
                None,
            ),
            (
                b"Content-Length: 3\r\n\r\n{}".to_vec(),
                ReadError::Truncated,
                None,
            ),
            (
                b"Content-Length: -1\r\n\r\n".to_vec(),
                ReadError::Unreadable,
                None,
            ),
        ] {
            let mut reader = std::io::Cursor::new(bytes);
            let error = read_frame(&mut reader).expect_err("a defective frame");
            assert_eq!(
                std::mem::discriminant(&error),
                std::mem::discriminant(&expected),
                "{error}"
            );
            if let Some(consumed) = consumed {
                assert_eq!(reader.position(), consumed);
            }
        }
    }
}
