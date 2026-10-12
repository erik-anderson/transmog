use crate::*;
use std::io::{Cursor, Read};
use transmog_capture::CaptureRecord;
use transmog_core::stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus};
use transmog_websocket::{Direction, FrameDecoder, FrameLimits};

fn message(sender: &str, kind: &str, payload: Vec<u8>) -> StreamMessage {
    let mut message = StreamMessage::new(sender, kind, payload.into());
    message.at_millis = 1_700_000_000_123;
    message
}

fn capture(h2: bool, messages: &[StreamMessage]) -> RecoveredCapture {
    let mut kinds = vec![
        CaptureRecordKind::RequestHead {
            boundary: ExchangeBoundary::ClientRequest,
            method: if h2 { "CONNECT" } else { "GET" }.into(),
            target: "https://example.test/chat?room=one".into(),
            headers: vec![
                connections::header("Host", "example.test"),
                connections::header("Sec-WebSocket-Extensions", "permessage-deflate"),
            ],
        },
        CaptureRecordKind::ResponseHead {
            boundary: ExchangeBoundary::ClientResponse,
            status: if h2 { 200 } else { 101 },
            headers: vec![
                connections::header("Upgrade", "websocket"),
                connections::header("Sec-WebSocket-Extensions", "permessage-deflate"),
            ],
        },
        CaptureRecordKind::StreamState(StreamState {
            protocol: StreamProtocol::WebSocket,
            status: StreamStatus::Open,
            at_millis: 1_700_000_000_000,
            reason: None,
        }),
        CaptureRecordKind::Completed,
    ];
    kinds.extend(
        messages
            .iter()
            .cloned()
            .map(CaptureRecordKind::StreamMessage),
    );
    RecoveredCapture {
        records: kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| CaptureRecord {
                exchange_id: 1,
                sequence: index as u64 + 1,
                kind,
            })
            .collect(),
        valid_bytes: 0,
        sealed: true,
        truncated_tail: false,
    }
}

fn export(
    capture: &RecoveredCapture,
    mode: SazMode,
    password: Option<transmog_capture::CapturePassword>,
) -> (Vec<u8>, SazReport) {
    let mut writer = SazExporter::new(Cursor::new(Vec::new()), mode, SazLimits::default()).unwrap();
    if let Some(password) = password {
        writer = writer.encrypted(password).unwrap();
    }
    writer.export(capture).unwrap();
    let report = writer.report().unwrap();
    (writer.into_inner().unwrap().into_inner(), report)
}

fn member(
    bytes: &[u8],
    name: &str,
    password: Option<&transmog_capture::CapturePassword>,
) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut file = if let Some(password) = password {
        archive.by_name_decrypt(name, password.bytes()).unwrap()
    } else {
        archive.by_name(name).unwrap()
    };
    let mut result = Vec::new();
    file.read_to_end(&mut result).unwrap();
    result
}

// Parse Fiddler's envelope independently, then pass each frame to the RFC decoder.
fn frames(mut bytes: &[u8]) -> Vec<(u64, bool, transmog_websocket::Frame)> {
    assert!(bytes.starts_with(b"\r\n"));
    bytes = &bytes[2..];
    let mut frames = Vec::new();
    while !bytes.is_empty() {
        let end = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap();
        let header = std::str::from_utf8(&bytes[..end]).unwrap();
        let lines = header
            .split("\r\n")
            .map(|line| line.split_once(": ").unwrap())
            .collect::<BTreeMap<_, _>>();
        let outbound = lines.contains_key("Request-Length");
        let length: usize = lines[if outbound {
            "Request-Length"
        } else {
            "Response-Length"
        }]
        .parse()
        .unwrap();
        let id = lines["ID"].parse().unwrap();
        assert_eq!(lines["DoneRead"], "2023-11-14T22:13:20.123Z");
        assert_eq!(lines["BeginSend"], "0001-01-01T00:00:00");
        let direction = if outbound {
            Direction::ClientToServer
        } else {
            Direction::ServerToClient
        };
        let mut decoder = FrameDecoder::new(direction, FrameLimits::default(), false).unwrap();
        decoder.push(&bytes[end + 4..end + 4 + length]).unwrap();
        let frame = decoder.next_frame().unwrap().unwrap();
        assert!(frame.fin && !frame.compressed);
        assert!(decoder.next_frame().unwrap().is_none());
        frames.push((id, outbound, frame));
        bytes = &bytes[end + 4 + length..];
        assert!(bytes.starts_with(b"\r\n"));
        bytes = &bytes[2..];
    }
    frames
}

#[test]
fn websocket_saz_exports_frames_in_both_profiles_and_encrypted_archives() {
    let mut close = message("client", "close", b"bye".to_vec());
    close.close_code = Some(1000);
    let mut compressed = message("server", "text", "Hello 🌍".as_bytes().to_vec());
    compressed.compressed = true;
    let messages = vec![
        message("client", "text", b"test".to_vec()),
        compressed,
        message("client", "binary", vec![0, 255, 13, 10, 0]),
        message("server", "ping", b"ping".to_vec()),
        message("client", "pong", Vec::new()),
        close,
    ];
    for h2 in [false, true] {
        for mode in [SazMode::Strict, SazMode::Extended] {
            for encrypted in [false, true] {
                let password = encrypted.then(|| {
                    transmog_capture::CapturePassword::new("Transmog-interop-password".into())
                });
                let (bytes, report) = export(&capture(h2, &messages), mode, password.clone());
                assert_eq!(report.entries, if mode == SazMode::Strict { 6 } else { 8 });
                let decoded = frames(&member(&bytes, "raw/1_w.txt", password.as_ref()));
                assert_eq!(decoded.len(), messages.len());
                for (index, ((id, outbound, frame), expected)) in
                    decoded.iter().zip(&messages).enumerate()
                {
                    assert_eq!(*id, index as u64 + 1);
                    assert_eq!(*outbound, expected.sender == "client");
                    let payload = if expected.kind == "close" {
                        [&1000_u16.to_be_bytes()[..], &expected.payload].concat()
                    } else {
                        expected.payload.to_vec()
                    };
                    assert_eq!(&frame.payload[..], payload);
                }
                let request =
                    String::from_utf8(member(&bytes, "raw/1_c.txt", password.as_ref())).unwrap();
                let response =
                    String::from_utf8(member(&bytes, "raw/1_s.txt", password.as_ref())).unwrap();
                assert!(request.starts_with("GET https://example.test/chat?room=one HTTP/1.1\r\n"));
                assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
                assert!(
                    !request
                        .to_ascii_lowercase()
                        .contains("sec-websocket-extensions")
                );
                assert!(
                    !response
                        .to_ascii_lowercase()
                        .contains("sec-websocket-extensions")
                );
                let metadata =
                    String::from_utf8(member(&bytes, "raw/1_m.xml", password.as_ref())).unwrap();
                assert!(metadata.contains("BitFlags=\"40401\""));
                assert!(metadata.contains("WebSocket messages are included"));
                if let Some(directory) = std::env::var_os("TRANSMOG_SAZ_INTEROP_DIR") {
                    let name = format!(
                        "websocket-{}-{}-{}.saz",
                        if h2 { "h2" } else { "h1" },
                        if mode == SazMode::Strict {
                            "strict"
                        } else {
                            "extended"
                        },
                        if encrypted { "encrypted" } else { "plain" }
                    );
                    std::fs::write(std::path::PathBuf::from(directory).join(name), bytes).unwrap();
                }
            }
        }
    }
}

#[test]
fn websocket_saz_retains_large_messages_and_histories_without_http_body_caps() {
    let mut messages = [0, 125, 126, 65535, 65536, 24 * 1024 * 1024]
        .into_iter()
        .map(|length| message("client", "binary", vec![0xab; length]))
        .collect::<Vec<_>>();
    messages.extend(
        (0..1100).map(|_| message("server", "text", b"next\r\nRequest-Length: 9".to_vec())),
    );
    let capture = capture(false, &messages);
    let mut writer = SazExporter::new(
        Cursor::new(Vec::new()),
        SazMode::Strict,
        SazLimits {
            max_body_bytes_per_direction: 1,
            ..SazLimits::default()
        },
    )
    .unwrap();
    writer.export(&capture).unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();
    let decoded = frames(&member(&bytes, "raw/1_w.txt", None));
    assert_eq!(decoded.len(), messages.len());
    for ((_, _, actual), expected) in decoded.iter().zip(messages) {
        assert_eq!(actual.payload, expected.payload);
    }
}

#[test]
fn incomplete_websocket_observations_are_disclosed_and_invalid_frames_rejected() {
    let mut incomplete = message("client", "text", b"prefix".to_vec());
    incomplete.truncated = true;
    let diagnostic = message("server", "unavailable", b"decoder failed".to_vec());
    let (bytes, _) = export(
        &capture(false, &[incomplete, diagnostic]),
        SazMode::Strict,
        None,
    );
    assert!(frames(&member(&bytes, "raw/1_w.txt", None)).is_empty());
    assert!(
        String::from_utf8(member(&bytes, "raw/1_m.xml", None))
            .unwrap()
            .contains("2 incomplete or unavailable")
    );
    for invalid in [
        message("client", "ping", vec![0; 126]),
        message("server", "close", vec![0; 1024]),
        message("server", "text", vec![255]),
        message("bad\r\nheader", "text", b"x".to_vec()),
        message("server", "close", b"no code".to_vec()),
    ] {
        let mut writer = SazExporter::new(
            Cursor::new(Vec::new()),
            SazMode::Strict,
            SazLimits::default(),
        )
        .unwrap();
        assert!(matches!(
            writer.export(&capture(false, &[invalid])),
            Err(SazError::InvalidArchive)
        ));
    }
    let mut writer = SazExporter::new(
        Cursor::new(Vec::new()),
        SazMode::Strict,
        SazLimits {
            max_entries: 5,
            ..SazLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        writer.export(&capture(false, &[])),
        Err(SazError::EntryLimitExceeded)
    ));
}

#[test]
fn rejected_handshakes_and_sse_keep_their_http_identity() {
    for sse in [false, true] {
        let diagnostic = message("server", "unavailable", b"observation unavailable".to_vec());
        let mut capture = capture(false, &[diagnostic]);
        for record in &mut capture.records {
            match &mut record.kind {
                CaptureRecordKind::ResponseHead {
                    status, headers, ..
                } => {
                    *status = if sse { 200 } else { 403 };
                    *headers = vec![connections::header(
                        "Content-Type",
                        if sse {
                            "text/event-stream"
                        } else {
                            "text/plain"
                        },
                    )];
                }
                CaptureRecordKind::StreamState(state) => {
                    state.protocol = if sse {
                        StreamProtocol::Sse
                    } else {
                        StreamProtocol::WebSocket
                    };
                    state.status = if sse {
                        StreamStatus::Open
                    } else {
                        StreamStatus::Failed
                    };
                }
                _ => {}
            }
        }
        // Stream records can precede HTTP heads when saving a desktop snapshot.
        capture.records.rotate_right(2);
        let (bytes, report) = export(&capture, SazMode::Strict, None);
        assert_eq!(report.entries, 5);
        let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        assert!(archive.by_name("raw/1_w.txt").is_err());
        let response = String::from_utf8(member(&bytes, "raw/1_s.txt", None)).unwrap();
        assert!(response.starts_with(if sse {
            "HTTP/1.1 200 "
        } else {
            "HTTP/1.1 403 "
        }));
        let metadata = String::from_utf8(member(&bytes, "raw/1_m.xml", None)).unwrap();
        assert!(metadata.contains("BitFlags=\"401\""));
        assert!(metadata.contains(if sse {
            "Parsed SSE events are omitted"
        } else {
            "handshake did not establish"
        }));
    }
}
