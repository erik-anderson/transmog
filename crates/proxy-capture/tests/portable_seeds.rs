//! Current-format native evidence seeds for the portable-case design corpus.
use std::{fs, io::Cursor, path::PathBuf};
use transmog_capture::{
    CaptureEncoding, CaptureLimits, CapturePassword, CaptureReader, CaptureRecord,
    CaptureRecordKind, CaptureWriter, CapturedHeader,
};
use transmog_core::{
    observe::ExchangeBoundary,
    stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus},
};

const PASSWORD: &str = "portable-fixture-password";

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../verification/fixtures/portable/seeds")
}

fn header(name: &str, value: &[u8]) -> CapturedHeader {
    CapturedHeader {
        name: name.as_bytes().to_vec(),
        value: Some(value.to_vec()),
        original_value_bytes: Some(value.len()),
    }
}

fn response_body(bytes: &[u8], observed: usize, truncated: bool) -> CaptureRecordKind {
    CaptureRecordKind::BodySegment {
        boundary: ExchangeBoundary::ClientResponse,
        byte_count: observed,
        bytes: Some(bytes.to_vec()),
        truncated,
    }
}

fn request(target: &str, headers: Vec<CapturedHeader>) -> CaptureRecordKind {
    CaptureRecordKind::RequestHead {
        boundary: ExchangeBoundary::ClientRequest,
        method: "GET".into(),
        target: format!("https://portable.invalid/{target}"),
        headers,
    }
}

fn event_message() -> StreamMessage {
    let mut message = StreamMessage::new("server", "event", b"value=42".as_slice().into());
    message.at_millis = 1_800_000_000_005;
    message.event = Some("update".into());
    message.event_id = Some("seed-event".into());
    message.raw = Some("id: seed-event\nevent: update\ndata: value=42\n\n".into());
    message
}

fn records(alternative: bool) -> Vec<CaptureRecord> {
    let css = if alternative {
        b"h1 { color: blue; }\n".as_slice()
    } else {
        b"h1 { color: green; }\n".as_slice()
    };
    let data = [
        (
            1,
            request("style.css", vec![header("X-Client", b"fixture")]),
        ),
        (
            1,
            CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status: 200,
                headers: vec![
                    header("Content-Type", b"text/css"),
                    header("X-Duplicate", b"one"),
                    header("x-duplicate", b"two"),
                    header("X-Bytes", &[0x80, 0xff]),
                ],
            },
        ),
        (1, response_body(css, css.len(), false)),
        (1, CaptureRecordKind::Completed),
        (2, request("incomplete", vec![])),
        (
            2,
            CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status: 200,
                headers: vec![header("Content-Length", b"100")],
            },
        ),
        (2, response_body(b"prefix", 100, true)),
        (
            2,
            CaptureRecordKind::Failed {
                category: "synthetic-interruption".into(),
                message: "Synthetic retained prefix".into(),
            },
        ),
        (3, request("events", vec![])),
        (
            3,
            CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status: 200,
                headers: vec![header("Content-Type", b"text/event-stream")],
            },
        ),
        (
            3,
            CaptureRecordKind::StreamState(StreamState {
                protocol: StreamProtocol::Sse,
                status: StreamStatus::Open,
                at_millis: 1_800_000_000_000,
                reason: None,
            }),
        ),
        (3, CaptureRecordKind::StreamMessage(event_message())),
        (
            3,
            CaptureRecordKind::StreamState(StreamState {
                protocol: StreamProtocol::Sse,
                status: StreamStatus::Interrupted,
                at_millis: 1_800_000_000_010,
                reason: Some("Synthetic stop".into()),
            }),
        ),
        (3, CaptureRecordKind::Completed),
        (
            0,
            CaptureRecordKind::Loss {
                first_missing_sequence: 99,
                count: 2,
                reason: "synthetic-buffer-eviction".into(),
            },
        ),
    ];
    data.into_iter()
        .enumerate()
        .map(|(index, (exchange_id, kind))| CaptureRecord {
            sequence: u64::try_from(index).unwrap() + 1,
            exchange_id,
            kind,
        })
        .collect()
}

fn load(bytes: &[u8], password: Option<&CapturePassword>) -> Vec<CaptureRecord> {
    let mut reader =
        CaptureReader::with_password(Cursor::new(bytes), CaptureLimits::default(), password)
            .unwrap();
    let mut loaded = vec![];
    while let Some(frame) = reader.read_next().unwrap() {
        if !matches!(frame.record.kind, CaptureRecordKind::Seal { .. }) {
            loaded.push(frame.record);
        }
    }
    assert!(reader.sealed());
    assert!(!reader.truncated_tail());
    loaded
}

#[test]
fn checked_in_portable_seeds_retain_collisions_bytes_gaps_and_streams() {
    let first = fs::read(directory().join("source-a.tmcap")).unwrap();
    let other = fs::read(directory().join("source-b.tmcap")).unwrap();
    assert_ne!(first, other);
    assert_eq!(load(&first, None), records(false));
    assert_eq!(load(&other, None), records(true));
    assert_eq!(
        load(&first, None)[0].exchange_id,
        load(&other, None)[0].exchange_id
    );
}

#[test]
fn encrypted_portable_seed_requires_password_and_preserves_all_evidence() {
    let encrypted = fs::read(directory().join("encrypted.tmcap")).unwrap();
    assert!(CaptureReader::new(Cursor::new(&encrypted), CaptureLimits::default()).is_err());
    let wrong = CapturePassword::new("wrong fixture password".into());
    assert!(
        CaptureReader::with_password(
            Cursor::new(&encrypted),
            CaptureLimits::default(),
            Some(&wrong)
        )
        .is_err()
    );
    let password = CapturePassword::new(PASSWORD.into());
    assert_eq!(load(&encrypted, Some(&password)), records(false));
}

#[test]
#[ignore = "Explicit fixture regeneration; follow verification/fixtures/portable/README.md"]
fn regenerate_portable_capture_seeds() {
    fs::create_dir_all(directory()).unwrap();
    for (name, alternative, encrypted) in [
        ("source-a.tmcap", false, false),
        ("source-b.tmcap", true, false),
        ("encrypted.tmcap", false, true),
    ] {
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding {
                password: encrypted.then(|| CapturePassword::new(PASSWORD.into())),
            },
        )
        .unwrap();
        for record in records(alternative) {
            writer.append(&record).unwrap();
        }
        writer.seal().unwrap();
        fs::write(directory().join(name), writer.into_inner().unwrap()).unwrap();
    }
}
