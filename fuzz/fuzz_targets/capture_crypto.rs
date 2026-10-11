#![no_main]
use libfuzzer_sys::fuzz_target;
use std::{io::Cursor, sync::OnceLock};
use transmog_capture::{
    CaptureEncoding, CaptureFrame, CaptureLimits, CapturePassword, CaptureReader, CaptureRecord,
    CaptureRecordKind, CaptureWriter, read_indexed_frame_with_codec,
};

struct Seed {
    bytes: Vec<u8>,
    frame: CaptureFrame,
    expected: CaptureRecord,
    start: usize,
    block: usize,
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
        let block = start + (frame.offset as usize - start) / 1040 * 1040;
        Seed {
            bytes,
            frame,
            expected,
            start,
            block,
        }
    })
}
fuzz_target!(|input: &[u8]| {
    if input.len() < 4 {
        return;
    }
    let seed = seed();
    let mut bytes = seed.bytes.clone();
    let mut offset = seed.frame.offset;
    let mut index = seed.frame.index;
    let mut length = seed.frame.frame_bytes;
    match input[0] % 4 {
        0 => {
            // Every single-bit ciphertext/tag mutation in the selected block must fail.
            let p = seed.block + u16::from_le_bytes([input[1], input[2]]) as usize % 1040;
            bytes[p] ^= 1 << (input[3] % 8);
        }
        1 => index = index.saturating_add(1 + u64::from(input[1])),
        2 => length = if input[1] & 1 == 0 { u64::MAX } else { 0 },
        _ => {
            // Header/index replacement cannot turn the pinned context into a new key.
            let p = u16::from_le_bytes([input[1], input[2]]) as usize % seed.start;
            bytes[p] ^= 1 << (input[3] % 8);
            offset = seed.frame.offset;
        }
    }
    let mut cursor = Cursor::new(bytes);
    cursor.set_position(offset);
    let result = read_indexed_frame_with_codec(
        cursor,
        length,
        CaptureLimits::default(),
        &seed.frame.codec,
        index,
    );
    if input[0] % 4 == 3 {
        assert_eq!(result.unwrap(), seed.expected);
    } else {
        assert!(result.is_err());
    }
});
