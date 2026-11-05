//! Shared protocol fuzz oracles, also replayed by ordinary Windows tests.

use serde::{Serialize, de::DeserializeOwned};
use std::io::{Cursor, Read};
use transmog_extensions::{HostReply, HostRequest, MAX_HOST_FRAME_BYTES, read_frame, write_frame};
use transmog_script::{
    MAX_SCRIPT_HOST_FRAME_BYTES, ScriptHostReply, ScriptHostRequest, read_host_frame,
    write_host_frame,
};

const MAX_INPUT: usize = 16 * 1024;

struct Fragmented<'a> {
    remaining: &'a [u8],
    chunk: usize,
}
impl Read for Fragmented<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = output.len().min(self.remaining.len()).min(self.chunk);
        output[..count].copy_from_slice(&self.remaining[..count]);
        self.remaining = &self.remaining[count..];
        Ok(count)
    }
}

#[derive(Clone, Copy)]
enum Protocol {
    Script,
    Extension,
}

fn decode<T: DeserializeOwned>(protocol: Protocol, bytes: &[u8], chunk: usize) -> Option<T> {
    let mut input = Fragmented {
        remaining: bytes,
        chunk,
    };
    match protocol {
        Protocol::Script => read_host_frame(&mut input).ok(),
        Protocol::Extension => read_frame(&mut input).ok(),
    }
}

fn encode<T: Serialize>(protocol: Protocol, value: &T) -> Vec<u8> {
    let mut output = Vec::new();
    match protocol {
        Protocol::Script => write_host_frame(&mut output, value).unwrap(),
        Protocol::Extension => write_frame(&mut output, value).unwrap(),
    }
    output
}

fn roundtrip<T: DeserializeOwned + Serialize>(
    protocol: Protocol,
    bytes: &[u8],
    chunk: usize,
) -> bool {
    let Some(value) = decode::<T>(protocol, bytes, chunk) else {
        return false;
    };
    let frame = encode(protocol, &value);
    let roundtrip: T = decode(protocol, &frame, 1).expect("fragmented canonical frame");
    assert_eq!(
        serde_json::to_value(&value).unwrap(),
        serde_json::to_value(&roundtrip).unwrap()
    );
    // A reader must consume exactly one frame, leaving the next frame untouched.
    let mut pair = frame.clone();
    pair.extend_from_slice(&frame);
    let mut pair = Cursor::new(pair);
    let _: T = match protocol {
        Protocol::Script => read_host_frame(&mut pair).unwrap(),
        Protocol::Extension => read_frame(&mut pair).unwrap(),
    };
    assert_eq!(pair.position(), frame.len() as u64);
    // The boundary at either end and a variable interior prefix must all fail.
    for length in [0, 3, frame.len() / 2, frame.len() - 1] {
        assert!(decode::<T>(protocol, &frame[..length.min(frame.len())], chunk).is_none());
    }
    true
}

fn framed(payload: &[u8]) -> Vec<u8> {
    let mut frame = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
    frame.extend_from_slice(payload);
    frame
}

fn raw(bytes: &[u8], chunk: usize) {
    roundtrip::<ScriptHostRequest>(Protocol::Script, bytes, chunk);
    roundtrip::<ScriptHostReply>(Protocol::Script, bytes, chunk);
    roundtrip::<HostRequest>(Protocol::Extension, bytes, chunk);
    roundtrip::<HostReply>(Protocol::Extension, bytes, chunk);
    stream_roundtrip(bytes, chunk);
}

fn stream_roundtrip(bytes: &[u8], chunk: usize) {
    macro_rules! stream {
        ($read:path,$write:path) => {
            let mut input = Fragmented {
                remaining: bytes,
                chunk,
            };
            if let Ok(value) = $read(&mut input) {
                let mut encoded = Vec::new();
                $write(&mut encoded, &value).unwrap();
                let mut fragmented = Fragmented {
                    remaining: &encoded,
                    chunk: 1,
                };
                let decoded = $read(&mut fragmented).unwrap();
                assert_eq!(
                    serde_json::to_value(&value).unwrap(),
                    serde_json::to_value(decoded).unwrap()
                );
                // Changing the high-bit class must fail even with valid JSON.
                encoded[3] ^= 0x80;
                assert!($read(&mut Cursor::new(encoded)).is_err());
            }
        };
    }
    stream!(
        transmog_script::read_host_request_frame,
        transmog_script::write_host_request_frame
    );
    stream!(
        transmog_script::read_host_reply_frame,
        transmog_script::write_host_reply_frame
    );
    stream!(
        transmog_extensions::read_request,
        transmog_extensions::write_request
    );
    stream!(
        transmog_extensions::read_reply,
        transmog_extensions::write_reply
    );
}

struct PrefixOnly {
    prefix: Cursor<[u8; 4]>,
}
impl Read for PrefixOnly {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        assert!(
            self.prefix.position() < 4,
            "invalid lengths must fail before reading/allocating payloads"
        );
        self.prefix.read(output)
    }
}

/// Mutates framing, typed envelopes and strict field validation without running JS.
pub fn check(data: &[u8]) {
    if data.len() > MAX_INPUT {
        return;
    }
    let mode = data.first().copied().unwrap_or(0) % 10;
    let payload = data.get(1..).unwrap_or_default();
    let chunk = usize::from(payload.first().copied().unwrap_or(0) % 17) + 1;
    let token = [payload.first().copied().unwrap_or(0); 32];
    let text = String::from_utf8_lossy(payload);
    match mode {
        0 => raw(payload, chunk),
        1 => raw(&framed(payload), chunk),
        2 => {
            let value = serde_json::json!({"message":"ready", "protocol_version":1,"token":token});
            assert!(roundtrip::<ScriptHostReply>(
                Protocol::Script,
                &framed(&serde_json::to_vec(&value).unwrap()),
                chunk
            ));
        }
        3 => {
            let value =
                serde_json::json!({"message":"shutdown", "protocol_version":1,"token":token});
            assert!(roundtrip::<ScriptHostRequest>(
                Protocol::Script,
                &framed(&serde_json::to_vec(&value).unwrap()),
                chunk
            ));
        }
        4 => {
            let value = serde_json::json!({"kind":"payload", "token":token,"path":text,"offset":"9007199254740993","bytes_base64":text});
            assert!(roundtrip::<HostRequest>(
                Protocol::Extension,
                &framed(&serde_json::to_vec(&value).unwrap()),
                chunk
            ));
        }
        5 => {
            let value = serde_json::json!({"kind":"result", "token":token,"request_id":u64::MAX,
                "result":{"Ok":{"kind":"needInput","reads":[],"token":text}}});
            assert!(roundtrip::<HostReply>(
                Protocol::Extension,
                &framed(&serde_json::to_vec(&value).unwrap()),
                chunk
            ));
        }
        6 => {
            let mut value = serde_json::json!({"kind":"ready", "token":token,"unexpected":text});
            assert!(
                decode::<HostReply>(
                    Protocol::Extension,
                    &framed(&serde_json::to_vec(&value).unwrap()),
                    chunk
                )
                .is_none()
            );
            value["kind"] = "futureReply".into();
            assert!(
                decode::<HostReply>(
                    Protocol::Extension,
                    &framed(&serde_json::to_vec(&value).unwrap()),
                    chunk
                )
                .is_none()
            );
            let duplicate = format!(
                "{{\"kind\":\"ready\",\"token\":{},\"token\":{}}}",
                serde_json::to_string(&token).unwrap(),
                serde_json::to_string(&token).unwrap()
            );
            assert!(
                decode::<HostReply>(Protocol::Extension, &framed(duplicate.as_bytes()), chunk)
                    .is_none()
            );
        }
        8 => {
            use transmog_script::debugger::{DebugAction, DebugCommand};
            let command = serde_json::from_slice::<DebugCommand>(payload).unwrap_or(DebugCommand {
                session: "a".repeat(32),
                request: 1,
                action: DebugAction::Scopes { pause: 1, frame: 0 },
            });
            let request = ScriptHostRequest::ControlDebugger {
                protocol_version: transmog_script::SCRIPT_HOST_PROTOCOL_VERSION,
                token: transmog_script::ScriptHostToken(token),
                command,
            };
            let mut bytes = Vec::new();
            transmog_script::write_host_request_frame(&mut bytes, &request).unwrap();
            stream_roundtrip(&bytes, chunk);
        }
        9 => {
            use transmog_script::debugger::{DebugEvent, DebugEventKind};
            let event = serde_json::from_slice::<DebugEvent>(payload).unwrap_or(DebugEvent {
                value_generation: None,
                session: "b".repeat(32),
                invocation: u64::MAX,
                request: 0,
                event: DebugEventKind::Stopped {
                    reason: text.into_owned(),
                },
            });
            let reply = ScriptHostReply::Debugger {
                protocol_version: transmog_script::SCRIPT_HOST_PROTOCOL_VERSION,
                token: transmog_script::ScriptHostToken(token),
                event,
            };
            let mut bytes = Vec::new();
            transmog_script::write_host_reply_frame(&mut bytes, &reply).unwrap();
            stream_roundtrip(&bytes, chunk);
        }
        _ => {
            for length in [0, MAX_SCRIPT_HOST_FRAME_BYTES as u32 + 1, u32::MAX] {
                let mut input = PrefixOnly {
                    prefix: Cursor::new(length.to_le_bytes()),
                };
                assert!(read_host_frame::<ScriptHostReply>(&mut input).is_err());
            }
            for length in [0, MAX_HOST_FRAME_BYTES as u32 + 1, u32::MAX] {
                let mut input = PrefixOnly {
                    prefix: Cursor::new(length.to_le_bytes()),
                };
                assert!(read_frame::<HostReply>(&mut input).is_err());
            }
            let mut input = PrefixOnly {
                prefix: Cursor::new((0x8000_0000u32 | 65537).to_le_bytes()),
            };
            assert!(transmog_script::read_host_request_frame(&mut input).is_err());
            let mut input = PrefixOnly {
                prefix: Cursor::new((0x8000_0000u32 | 65537).to_le_bytes()),
            };
            assert!(transmog_extensions::read_reply(&mut input).is_err());
        }
    }
}
