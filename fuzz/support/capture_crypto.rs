//! Encrypted `TMCap` fixtures and mutation oracles shared with seed regression tests.
use std::{io::Cursor, sync::OnceLock};
use transmog_capture::{
    CaptureEncoding, CaptureFrame, CaptureLimits, CapturePassword, CaptureReader, CaptureRecord,
    CaptureRecordKind, CaptureWriter, read_indexed_frame_with_codec,
};

struct Seed {
    bytes: Vec<u8>,
    frame: CaptureFrame,
    expected: CaptureRecord,
    performance: CaptureFrame,
    expected_performance: CaptureRecord,
    start: usize,
}
fn seed() -> &'static Seed {
    static SEED: OnceLock<Seed> = OnceLock::new();
    SEED.get_or_init(|| {
        let password = CapturePassword::new("fuzz-only-password".into());
        let expected = CaptureRecord {
            sequence: 1,
            exchange_id: 1,
            kind: CaptureRecordKind::BodySegment {
                boundary: transmog_core::observe::ExchangeBoundary::ClientResponse,
                byte_count: 2048,
                bytes: Some(vec![b'x'; 2048]),
                truncated: false,
            },
        };
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding {
                password: Some(password.clone()),
            },
        )
        .unwrap();
        writer.append(&expected).unwrap();
        let expected_performance = CaptureRecord {
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
        };
        writer.append(&expected_performance).unwrap();
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap();
        let header = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
        let start = 13 + header + 20 + 512;
        let mut reader = CaptureReader::with_password(
            Cursor::new(&bytes),
            CaptureLimits::default(),
            Some(&password),
        )
        .unwrap();
        let frame = reader.read_next_indexed().unwrap().unwrap();
        reader.read_next_indexed().unwrap().unwrap(); // Shared definition.
        let performance = reader.read_next_indexed().unwrap().unwrap();
        Seed {
            bytes,
            frame,
            expected,
            performance,
            expected_performance,
            start,
        }
    })
}
pub(crate) fn exercise(input: &[u8]) {
    if input.len() < 4 {
        return;
    }
    let seed = seed();
    let (frame, expected) = if input[0] & 4 == 0 {
        (&seed.frame, &seed.expected)
    } else {
        (&seed.performance, &seed.expected_performance)
    };
    let block = seed.start + (usize::try_from(frame.offset).unwrap() - seed.start) / 1040 * 1040;
    let mut bytes = seed.bytes.clone();
    let mut offset = frame.offset;
    let mut index = frame.index;
    let mut length = frame.frame_bytes;
    match input[0] % 4 {
        0 => {
            // Every single-bit ciphertext/tag mutation in the selected block must fail.
            let p = block + u16::from_le_bytes([input[1], input[2]]) as usize % 1040;
            bytes[p] ^= 1 << (input[3] % 8);
        }
        1 => index = index.saturating_add(1 + u64::from(input[1])),
        2 => length = if input[1] & 1 == 0 { u64::MAX } else { 0 },
        _ => {
            // Header/index replacement cannot turn the pinned context into a new key.
            let p = u16::from_le_bytes([input[1], input[2]]) as usize % seed.start;
            bytes[p] ^= 1 << (input[3] % 8);
            offset = frame.offset;
        }
    }
    let mut cursor = Cursor::new(bytes);
    cursor.set_position(offset);
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
