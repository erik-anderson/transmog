#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Cursor, sync::OnceLock};
use transmog_capture::{
    CaptureLimits, CaptureReader, CaptureRecord, CaptureRecordKind, CaptureWriter,
    read_indexed_frame_with_codec,
};

const BLOCK: usize = 1024;
const PAYLOAD: usize = BLOCK - 5;

fn header() -> Vec<u8> {
    serde_json::to_vec(
        &json!({"version":1,"layout":"indexed-blocks","block_size":BLOCK,
        "compression":"zstd","cipher":"none","kdf":"none","salt":vec![0;16],
        "nonce_prefix":vec![0;4],"memory_kib":0,"iterations":0,"lanes":0}),
    )
    .unwrap()
}
fn object(bytes: &[u8]) -> Vec<u8> {
    let mut result = vec![0];
    result.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    result.extend_from_slice(bytes);
    result
}
fn file(blocks: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let header = header();
    let mut result = b"TMCAP001\0".to_vec();
    result.extend_from_slice(&(header.len() as u32).to_le_bytes());
    result.extend_from_slice(&header);
    result.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    result.extend_from_slice(&[0; 512]);
    for (ordinal, (kind, data)) in blocks.iter().enumerate() {
        assert!(data.len() <= PAYLOAD);
        let mut plain = vec![0; BLOCK];
        plain[0] = *kind;
        plain[1..5].copy_from_slice(&(data.len() as u32).to_le_bytes());
        plain[5..5 + data.len()].copy_from_slice(data);
        let mut hash = Sha256::new();
        hash.update(&header);
        hash.update((ordinal as u64).to_le_bytes());
        hash.update(b"TMCap block");
        hash.update(&plain);
        result.extend_from_slice(&plain);
        result.extend_from_slice(&hash.finalize()[..16]);
    }
    result
}
fn put(blocks: &mut Vec<(u8, Vec<u8>)>, kind: u8, bytes: &[u8]) {
    blocks.extend(bytes.chunks(PAYLOAD).map(|part| (kind, part.to_vec())));
}
fn frame(counter: u64, codec: u8, expanded: u32, data: &[u8]) -> Vec<u8> {
    let mut payload = counter.to_le_bytes().to_vec();
    payload.extend_from_slice(&expanded.to_le_bytes());
    payload.push(codec);
    payload.extend_from_slice(data);
    let mut result = (payload.len() as u32).to_le_bytes().to_vec();
    result.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    result.extend(payload);
    result
}
fn record_file(stored: Value, body: &[u8]) -> Vec<u8> {
    let metadata = serde_json::to_vec(&stored).unwrap();
    let mut data = frame(1, 0, metadata.len() as u32, &metadata);
    data.extend_from_slice(body);
    let start = 13 + header().len() + 4 + 512;
    let entries = json!([{"offset":start + 5,"frame_bytes":data.len(),"index":0,"stored":stored}]);
    let mut blocks = Vec::new();
    put(&mut blocks, 0, &data);
    let first_index = blocks.len();
    let index = object(&serde_json::to_vec(&entries).unwrap());
    put(&mut blocks, 1, &index);
    let footer = json!({"roots":[{"block":first_index,"blocks":blocks.len()-first_index,
        "bytes":index.len(),"first":0,"count":1,"level":0}],"count":1,
        "sealed":false,"block":blocks.len()});
    put(
        &mut blocks,
        3,
        &object(&serde_json::to_vec(&footer).unwrap()),
    );
    file(&blocks)
}
fn template(normal: bool) -> &'static [u8] {
    static TEMPLATE: OnceLock<Vec<u8>> = OnceLock::new();
    static NORMAL: OnceLock<Vec<u8>> = OnceLock::new();
    (if normal { &NORMAL } else { &TEMPLATE }).get_or_init(|| {
        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        writer
            .append(&CaptureRecord {
                sequence: 1,
                exchange_id: 1,
                kind: CaptureRecordKind::BodySegment {
                    boundary: transmog_core::observe::ExchangeBoundary::ClientResponse,
                    byte_count: 4096,
                    bytes: Some((0..4096).map(|i| (i * 31) as u8).collect()),
                    truncated: false,
                },
            })
            .unwrap();
        if normal {
            writer.checkpoint().unwrap();
        }
        writer.seal().unwrap();
        writer.into_inner().unwrap()
    })
}
fn mutate_template(data: &[u8], normal: bool) -> Vec<u8> {
    let mut bytes = template(normal).to_vec();
    let length = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
    let block_size =
        u32::from_le_bytes(bytes[13 + length..17 + length].try_into().unwrap()) as usize;
    let start = 13 + length + 4 + 512;
    for patch in data.chunks_exact(3) {
        let position =
            start + u16::from_le_bytes([patch[0], patch[1]]) as usize % (bytes.len() - start);
        bytes[position] ^= patch[2];
    }
    // Deliberately repair public checksums to exercise the post-integrity parser.
    let header = bytes[13..13 + length].to_vec();
    for (ordinal, block) in bytes[start..].chunks_exact_mut(block_size + 16).enumerate() {
        let mut hash = Sha256::new();
        hash.update(&header);
        hash.update((ordinal as u64).to_le_bytes());
        hash.update(b"TMCap block");
        hash.update(&block[..block_size]);
        block[block_size..].copy_from_slice(&hash.finalize()[..16]);
    }
    bytes
}
fn exercise(bytes: &[u8], control: u8) {
    let limits = CaptureLimits {
        max_file_bytes: 1024 * 1024,
        max_record_bytes: [128 * 1024, 16, 1024, 8192][(control as usize >> 4) % 4],
        max_records: 256,
    };
    let mut indexed = match CaptureReader::new(Cursor::new(bytes), limits) {
        Ok(reader) => reader,
        Err(_) => return,
    };
    let mut frames = Vec::new();
    loop {
        match indexed.read_next_indexed() {
            Ok(Some(frame)) => {
                assert!(frames.len() < limits.max_records);
                let mut cursor = Cursor::new(bytes);
                cursor.set_position(frame.offset);
                let _ = read_indexed_frame_with_codec(
                    cursor,
                    frame.frame_bytes,
                    limits,
                    &frame.codec,
                    frame.index,
                );
                frames.push(frame);
            }
            Ok(None) => break,
            Err(_) => return,
        }
    }
    // Every sealed capture contains its consistent final seal record.
    if indexed.sealed() {
        assert!(matches!(
            frames.last().map(|frame| &frame.record.kind),
            Some(CaptureRecordKind::Seal { .. })
        ));
    }
    let mut full = CaptureReader::new(Cursor::new(bytes), limits).unwrap();
    for expected in frames {
        let actual = match full.read_next() {
            Ok(Some(frame)) => frame,
            Err(_) => return, // Indexed opening deliberately skips body integrity.
            Ok(None) => panic!("full reader ended before metadata reader"),
        };
        let mut expected_record = expected.record;
        let mut actual_record = actual.record;
        if let CaptureRecordKind::BodySegment { bytes, .. } = &mut expected_record.kind {
            *bytes = None;
        }
        if let CaptureRecordKind::BodySegment { bytes, .. } = &mut actual_record.kind {
            *bytes = None;
        }
        assert_eq!(actual_record, expected_record);
    }
    assert!(full.read_next().unwrap().is_none());
}

fuzz_target!(|input: &[u8]| {
    let Some((&control, data)) = input.split_first() else {
        return;
    };
    if data.len() > 65536 {
        return;
    }
    let bytes = match control & 15 {
        0 => data.to_vec(),
        1 | 10 => mutate_template(data, control & 15 == 10),
        2 => file(&[(3, object(&data[..data.len().min(PAYLOAD - 5)]))]),
        3 => {
            let Ok(stored) = serde_json::from_slice::<Value>(data) else {
                return;
            };
            record_file(stored, &[])
        }
        4 => {
            let bytes = object(&data[..data.len().min(PAYLOAD - 5)]);
            let footer = json!({"roots":[{"block":1,"blocks":1,"bytes":bytes.len(),
                "first":0,"count":1,"level":1}],"count":1,"sealed":false,"block":2});
            file(&[
                (1, object(b"[]")),
                (2, bytes),
                (3, object(&serde_json::to_vec(&footer).unwrap())),
            ])
        }
        5 | 6 => {
            if data.len() < 8 {
                return;
            }
            let body = &data[8..];
            let mut encoded = zstd::bulk::compress(body, 1).unwrap();
            if !encoded.is_empty() {
                let p = data[0] as usize % encoded.len();
                encoded[p] ^= data[1];
            }
            let expanded = if control & 15 == 5 {
                body.len() as u32
            } else {
                u32::from_le_bytes(data[2..6].try_into().unwrap())
            };
            let encoded = frame(2 + u64::from(data[6]), data[7], expanded, &encoded);
            let digest: [u8; 32] = Sha256::digest(body).into();
            let stored = json!({"revision":3,"sequence":1,"exchange_id":1,"kind":"body-segment",
                "payload":{"boundary":"client-response","byte_count":body.len(),"bytes":null,"truncated":false},
                "body_index":{"frame_bytes":encoded.len(),"bytes":body.len(),"digest":digest}});
            record_file(stored, &encoded)
        }
        8 => {
            let digest = Sha256::digest(data)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            record_file(
                json!({"revision":3,"sequence":0,"exchange_id":0,"kind":"public-certificate",
                "payload":{"sha256":digest,"der":data}}),
                &[],
            )
        }
        9 => {
            let mut bytes = b"TMCAP001\0".to_vec();
            bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(data);
            bytes.extend_from_slice(&1024_u32.to_le_bytes());
            bytes.extend_from_slice(&[0; 512]);
            bytes
        }
        11 => {
            let Ok(Value::Array(mut entries)) = serde_json::from_slice::<Value>(data) else {
                return;
            };
            let start = 13 + header().len() + 4 + 512;
            for entry in &mut entries {
                if let Some(offset) = entry["offset"].as_u64() {
                    entry["offset"] = offset.saturating_add(start as u64).into();
                }
            }
            let index = object(&serde_json::to_vec(&entries).unwrap());
            let mut blocks = vec![(0, vec![0; PAYLOAD])];
            put(&mut blocks, 1, &index);
            let footer = json!({"roots":[{"block":1,"blocks":blocks.len()-1,"bytes":index.len(),
                "first":0,"count":entries.len(),"level":0}],"count":entries.len(),"sealed":false,"block":blocks.len()});
            put(
                &mut blocks,
                3,
                &object(&serde_json::to_vec(&footer).unwrap()),
            );
            file(&blocks)
        }
        _ => {
            // Raw compressed index object: correct envelope, hostile codec/length/zstd bytes.
            let data = &data[..data.len().min(PAYLOAD)];
            let footer = json!({"roots":[{"block":0,"blocks":1,"bytes":data.len(),"first":0,
                "count":1,"level":0}],"count":1,"sealed":false,"block":1});
            file(&[
                (1, data.to_vec()),
                (3, object(&serde_json::to_vec(&footer).unwrap())),
            ])
        }
    };
    exercise(&bytes, control);
});
