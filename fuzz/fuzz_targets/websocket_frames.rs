#![no_main]

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use transmog_websocket::{
    DataKind, Direction, Frame, FrameLimits, MessageDecoder, MessageEvent, PerMessageDeflate,
    PerMessageDeflateCodec, encode_frame,
};

const MAX_WIRE: usize = 8 * 1024;
const MAX_MESSAGE: usize = 16 * 1024;

fuzz_target!(|data: &[u8]| fuzz(data));

fn fuzz(data: &[u8]) {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let Some((&chunk_selector, payload)) = rest.split_first() else {
        return;
    };
    let direction = if flags & 1 == 0 {
        Direction::ClientToServer
    } else {
        Direction::ServerToClient
    };
    let negotiated = (flags & 2 != 0).then_some(PerMessageDeflate::default());
    let wire = if flags & 4 == 0 {
        decode_raw(payload)
    } else {
        structured_wire(payload, flags, direction, negotiated)
    };
    let limits = FrameLimits {
        max_frame_payload_bytes: MAX_WIRE,
        max_message_payload_bytes: MAX_MESSAGE,
        max_buffered_bytes: MAX_WIRE + 14,
    };
    let Ok(mut decoder) = MessageDecoder::new(direction, limits, negotiated) else {
        return;
    };
    let chunk_size = usize::from(chunk_selector % 64).saturating_add(1);
    for chunk in wire.chunks(chunk_size) {
        let events = match decoder.push(chunk) {
            Ok(events) => events,
            Err(_) => return,
        };
        for event in events {
            match event {
                MessageEvent::Message(message) => {
                    assert!(message.payload.len() <= MAX_MESSAGE);
                    if message.kind == DataKind::Text {
                        assert!(std::str::from_utf8(&message.payload).is_ok());
                    }
                }
                MessageEvent::Control(control) => match control {
                    transmog_websocket::ControlFrame::Close(close) => {
                        assert!(close.reason.len() <= 123);
                    }
                    transmog_websocket::ControlFrame::Ping(bytes)
                    | transmog_websocket::ControlFrame::Pong(bytes) => {
                        assert!(bytes.len() <= 125);
                    }
                },
            }
        }
    }
}

fn decode_raw(payload: &[u8]) -> Vec<u8> {
    let Some(hex) = payload.strip_prefix(b"hex:") else {
        return payload[..payload.len().min(MAX_WIRE)].to_vec();
    };
    hex.chunks_exact(2)
        .take(MAX_WIRE)
        .map_while(|pair| Some(hex_nibble(pair[0])? << 4 | hex_nibble(pair[1])?))
        .collect()
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn structured_wire(
    payload: &[u8],
    flags: u8,
    direction: Direction,
    negotiated: Option<PerMessageDeflate>,
) -> Vec<u8> {
    let text = flags & 8 != 0;
    let compress = flags & 16 != 0 && negotiated.is_some();
    let payload = &payload[..payload.len().min(MAX_WIRE / 2)];
    let wire_payload = if compress {
        let Ok(mut codec) =
            PerMessageDeflateCodec::new(negotiated.unwrap_or_default(), direction)
        else {
            return Vec::new();
        };
        match codec.compress(payload) {
            Ok(compressed) => compressed,
            Err(_) => return Vec::new(),
        }
    } else {
        payload.to_vec()
    };
    let opcode = if text { 1 } else { 2 };
    let mask = direction
        .requires_mask()
        .then_some([0x12, 0x34, 0x56, 0x78]);
    encode_frame(
        &Frame {
            fin: true,
            compressed: compress,
            opcode,
            payload: Bytes::from(wire_payload),
        },
        direction,
        mask,
    )
    .unwrap_or_default()
}
