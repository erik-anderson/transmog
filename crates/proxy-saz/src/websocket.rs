//! Fiddler's length-delimited _w.txt member, containing reconstructed RFC 6455 frames.
use crate::{CapturedRequest, CapturedResponse, SazError, Session, connections::header};
use std::{borrow::Cow, io::Write};
use transmog_core::stream::{StreamMessage, StreamProtocol};

pub(super) fn established(session: &Session) -> bool {
    session.websocket_messages.iter().any(|message| {
        matches!(
            message.kind.as_str(),
            "text" | "binary" | "close" | "ping" | "pong"
        )
    }) || session.response.as_ref().is_some_and(|response| {
        response.status == 101
            && response.headers.iter().any(|field| {
                field.name.eq_ignore_ascii_case(b"upgrade")
                    && field
                        .value
                        .as_ref()
                        .is_some_and(|value| value.eq_ignore_ascii_case(b"websocket"))
            })
            || session.stream == Some(StreamProtocol::WebSocket)
                && (200..300).contains(&response.status)
                && session
                    .request
                    .as_ref()
                    .is_some_and(|request| request.method == "CONNECT")
    })
}

pub(super) fn validate(session: &mut Session) -> Result<(), SazError> {
    for message in &session.websocket_messages {
        if prepare(message)?.is_none() {
            session.websocket_omitted += 1;
        }
    }
    Ok(())
}

// Normalized frames carry decoded content: remove compression negotiation from
// the displayed handshake. Preserve the original fields in extended evidence.
fn headers(
    original: &[transmog_capture::CapturedHeader],
    extended: bool,
) -> Vec<transmog_capture::CapturedHeader> {
    let mut fields = original
        .iter()
        .filter(|field| {
            ![
                b"sec-websocket-extensions".as_slice(),
                b"connection",
                b"upgrade",
                b"content-length",
                b"transfer-encoding",
            ]
            .iter()
            .any(|name| field.name.eq_ignore_ascii_case(name))
                && (!extended
                    || ![b"sec-websocket-key".as_slice(), b"sec-websocket-accept"]
                        .iter()
                        .any(|name| field.name.eq_ignore_ascii_case(name)))
        })
        .cloned()
        .collect::<Vec<_>>();
    fields.push(header("Connection", "Upgrade"));
    fields.push(header("Upgrade", "websocket"));
    fields
}

pub(super) fn request(session: &Session, original: &CapturedRequest) -> Option<CapturedRequest> {
    if !established(session) || session.request.is_none() {
        return None;
    }
    let extended = original.method == "CONNECT";
    let mut fields = headers(&original.headers, extended);
    if extended {
        // Archive-only RFC example nonce; this handshake is never sent to a peer.
        fields.push(header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="));
    }
    Some(CapturedRequest {
        method: if extended {
            "GET".into()
        } else {
            original.method.clone()
        },
        target: original.target.clone(),
        headers: fields,
    })
}

pub(super) fn response(session: &Session, original: &CapturedResponse) -> Option<CapturedResponse> {
    if !established(session) || session.response.is_none() {
        return None;
    }
    let extended = session
        .request
        .as_ref()
        .is_some_and(|request| request.method == "CONNECT");
    let mut fields = headers(&original.headers, extended);
    if extended {
        fields.push(header(
            "Sec-WebSocket-Accept",
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
        ));
    }
    Some(CapturedResponse {
        status: 101,
        headers: fields,
    })
}

struct Frame<'a> {
    outbound: bool,
    opcode: u8,
    payload: Cow<'a, [u8]>,
}

fn prepare(message: &StreamMessage) -> Result<Option<Frame<'_>>, SazError> {
    if matches!(message.kind.as_str(), "unavailable" | "incomplete")
        || message.truncated
        || message.byte_count != message.payload.len() as u64
    {
        return Ok(None);
    }
    let outbound = match message.sender.as_str() {
        "client" => true,
        "server" => false,
        _ => return Err(SazError::InvalidArchive),
    };
    let opcode = match message.kind.as_str() {
        "text" => 1,
        "binary" => 2,
        "close" => 8,
        "ping" => 9,
        "pong" => 10,
        _ => return Err(SazError::InvalidArchive),
    };
    if opcode >= 8 && message.payload.len() > 125 {
        return Err(SazError::InvalidArchive);
    }
    let payload = if opcode == 8 {
        let close = transmog_websocket::CloseFrame {
            code: message.close_code,
            reason: std::str::from_utf8(&message.payload)
                .map_err(|_| SazError::InvalidArchive)?
                .into(),
        };
        Cow::Owned(
            close
                .encode()
                .map_err(|_| SazError::InvalidArchive)?
                .to_vec(),
        )
    } else {
        if opcode == 1 && std::str::from_utf8(&message.payload).is_err() {
            return Err(SazError::InvalidArchive);
        }
        Cow::Borrowed(message.payload.as_ref())
    };
    Ok(Some(Frame {
        outbound,
        opcode,
        payload,
    }))
}

pub(super) fn write_messages(
    output: &mut impl Write,
    messages: &[StreamMessage],
) -> Result<(), SazError> {
    output.write_all(b"\r\n")?;
    let mut id = 0_u64;
    for message in messages {
        let Some(frame) = prepare(message)? else {
            continue;
        };
        id = id.checked_add(1).ok_or(SazError::EntryLimitExceeded)?;
        let length = u64::try_from(frame.payload.len()).map_err(|_| SazError::BodyLimitExceeded)?;
        if length > i64::MAX as u64 {
            return Err(SazError::BodyLimitExceeded);
        }
        let mut wire = Vec::with_capacity(14);
        wire.push(0x80 | frame.opcode);
        let mask = if frame.outbound { 0x80 } else { 0 };
        if length <= 125 {
            wire.push(mask | u8::try_from(length).unwrap());
        } else if let Ok(short) = u16::try_from(length) {
            wire.push(mask | 0x7e);
            wire.extend_from_slice(&short.to_be_bytes());
        } else {
            wire.push(mask | 0x7f);
            wire.extend_from_slice(&length.to_be_bytes());
        }
        if frame.outbound {
            wire.extend_from_slice(&[0; 4]);
        }
        let length = length
            .checked_add(wire.len() as u64)
            .ok_or(SazError::BodyLimitExceeded)?;
        let direction = if frame.outbound {
            "Request"
        } else {
            "Response"
        };
        let time = crate::metadata::timestamp(message.at_millis).ok_or(SazError::InvalidArchive)?;
        write!(
            output,
            "{direction}-Length: {length}\r\nID: {id}\r\nBitFlags: 0\r\nDoneRead: {time}\r\nBeginSend: 0001-01-01T00:00:00\r\nDoneSend: 0001-01-01T00:00:00\r\n\r\n"
        )?;
        output.write_all(&wire)?;
        // A zero mask leaves archive payloads unchanged, allowing bounded writes
        // without copying a potentially very large message into another buffer.
        output.write_all(&frame.payload)?;
        output.write_all(b"\r\n")?;
    }
    Ok(())
}
