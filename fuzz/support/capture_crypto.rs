//! Encrypted `TMCap` fixtures: both geometries, shared dictionaries, and every read block.
use std::{io::Cursor, sync::OnceLock};
use transmog_capture::{
    CaptureEncoding, CaptureFrame, CaptureLimits, CapturePassword, CaptureReader, CaptureRecord,
    CaptureRecordKind, CaptureWriter, read_indexed_frame_with_codec,
};
use transmog_core::observe::ExchangeBoundary;

struct Seed {
    bytes: Vec<u8>,
    frames: Vec<CaptureFrame>,
    expected: Vec<CaptureRecord>,
    start: usize,
    block_plain: usize,
}
fn seed(compact: bool) -> &'static Seed {
    static NORMAL: OnceLock<Seed> = OnceLock::new();
    static COMPACT: OnceLock<Seed> = OnceLock::new();
    (if compact { &COMPACT } else { &NORMAL }).get_or_init(|| {
        let password = CapturePassword::new("fuzz-only-password".into());
        let mut number = 123_456_789_u32;
        let base: Vec<u8> = (0..32 * 1024)
            .map(|_| {
                number ^= number << 13;
                number ^= number >> 17;
                number ^= number << 5;
                number.to_le_bytes()[0]
            })
            .collect();
        let mut changed = base.clone();
        changed[20_000] ^= 1;
        let mut expected = [
            (ExchangeBoundary::UpstreamResponse, base),
            (ExchangeBoundary::ClientResponse, changed),
        ]
        .map(|(boundary, bytes)| CaptureRecord {
            sequence: 1,
            exchange_id: 1,
            kind: CaptureRecordKind::BodySegment {
                boundary,
                byte_count: bytes.len(),
                bytes: Some(bytes),
                truncated: false,
            },
        })
        .to_vec();
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding {
                password: Some(password.clone()),
            },
        )
        .unwrap();
        for record in &expected {
            writer.append(record).unwrap();
        }
        expected.push(CaptureRecord {
            sequence: 2,
            exchange_id: 1,
            kind: CaptureRecordKind::Performance(transmog_core::performance::PerformanceEvidence {
                transports: vec![transmog_core::performance::TransportObservation {
                    leg: "upstream".into(),
                    connection_id: "private connection".into(),
                    peer: Some("127.0.0.1:443".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        });
        writer.append(expected.last().unwrap()).unwrap();
        for record in stream_records() {
            writer.append(&record).unwrap();
            expected.push(record);
        }
        if !compact {
            writer.checkpoint().unwrap();
        }
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap();
        let header = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
        let start = 13 + header + 20 + 512;
        let block_plain = if compact { 1024 } else { 16 * 1024 };
        assert!((bytes.len() - start).is_multiple_of(block_plain + 16));
        let mut reader = CaptureReader::with_password(
            Cursor::new(&bytes),
            CaptureLimits::default(),
            Some(&password),
        )
        .unwrap();
        let mut frames = Vec::new();
        while let Some(frame) = reader.read_next_indexed().unwrap() {
            if matches!(
                frame.record.kind,
                CaptureRecordKind::BodySegment { .. }
                    | CaptureRecordKind::Performance(_)
                    | CaptureRecordKind::StreamMessage(_)
            ) {
                frames.push(frame);
            }
        }
        assert_eq!(frames.len(), 5);
        assert!(
            frames[1].frame_bytes < 1024,
            "fixture must use a dictionary"
        );
        Seed {
            bytes,
            frames,
            expected,
            start,
            block_plain,
        }
    })
}
fn stream_records() -> [CaptureRecord; 2] {
    let id: std::sync::Arc<str> = "persistent SSE identity".repeat(128).into();
    [3, 4].map(|sequence| {
        let mut message = transmog_core::stream::StreamMessage::new(
            "server",
            "event",
            b"encrypted stream payload".to_vec().into(),
        );
        message.event_id = Some(id.clone());
        message.raw = Some("data: encrypted stream payload\n\n".into());
        CaptureRecord {
            sequence,
            exchange_id: 1,
            kind: CaptureRecordKind::StreamMessage(message),
        }
    })
}
fn object_blocks(seed: &Seed, frame: &CaptureFrame, blocks: &mut Vec<usize>) {
    let stride = seed.block_plain + 16;
    let relative = usize::try_from(frame.offset).unwrap() - seed.start;
    let first = relative / stride;
    let within = relative % stride - 5;
    let last =
        first + (within + usize::try_from(frame.frame_bytes).unwrap() - 1) / (seed.block_plain - 5);
    blocks.extend((first..=last).map(|ordinal| seed.start + ordinal * stride));
}
pub(crate) fn exercise(input: &[u8]) {
    if input.len() < 4 {
        return;
    }
    let seed = seed(input[0] & 8 != 0);
    let selected = if input[0] & 32 != 0 {
        3 + usize::from(input[0] & 16 != 0)
    } else if input[0] & 4 != 0 {
        2
    } else {
        usize::from(input[0] & 16 != 0)
    };
    let frame = &seed.frames[selected];
    let expected = &seed.expected[selected];
    let mut blocks = Vec::new();
    object_blocks(seed, frame, &mut blocks);
    // All random reads load the canonical metadata page. Dependent bodies also
    // read the independent base, potentially spanning many physical blocks.
    object_blocks(seed, &seed.frames[2], &mut blocks);
    if selected == 1 {
        object_blocks(seed, &seed.frames[0], &mut blocks);
    }
    blocks.sort_unstable();
    blocks.dedup();
    let block = blocks[usize::from(input.get(4).copied().unwrap_or(0)) % blocks.len()];
    let mut bytes = seed.bytes.clone();
    let mut index = frame.index;
    let mut length = frame.frame_bytes;
    match input[0] % 4 {
        0 => {
            let position = block
                + usize::from(u16::from_le_bytes([input[1], input[2]])) % (seed.block_plain + 16);
            bytes[position] ^= 1 << (input[3] % 8);
        }
        1 => index = index.saturating_add(1 + u64::from(input[1])),
        2 => length = if input[1] & 1 == 0 { u64::MAX } else { 0 },
        _ => {
            let position = usize::from(u16::from_le_bytes([input[1], input[2]])) % seed.start;
            bytes[position] ^= 1 << (input[3] % 8);
        }
    }
    let mut cursor = Cursor::new(bytes);
    cursor.set_position(frame.offset);
    let result = read_indexed_frame_with_codec(
        cursor,
        length,
        CaptureLimits::default(),
        &frame.codec,
        index,
    );
    if input[0] % 4 == 3 {
        assert_eq!(&result.unwrap(), expected);
    } else {
        assert!(result.is_err());
    }
}
