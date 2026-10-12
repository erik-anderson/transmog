//! `TMCap` fuzz fixture builders shared with semantic seed regression tests.
// Inputs and synthesized files are bounded well below u32::MAX by this harness.
#![allow(clippy::cast_possible_truncation)]
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
fn packed(kind: u8, codec: u8, expanded: u32, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![kind, codec];
    bytes.extend_from_slice(&expanded.to_le_bytes());
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(data);
    bytes
}
fn page(kind: u8, data: &[u8]) -> Vec<u8> {
    packed(kind, 0, data.len() as u32, data)
}
fn start() -> usize {
    13 + header().len() + 4 + 512
}
fn record_file(stored: Value, body: &[u8]) -> Vec<u8> {
    record_files(&[(stored, body.to_vec())])
}
fn record_files(records: &[(Value, Vec<u8>)]) -> Vec<u8> {
    records_with_refs(records, |_| {})
}
fn records_with_refs(records: &[(Value, Vec<u8>)], edit: impl FnOnce(&mut Vec<Value>)) -> Vec<u8> {
    let mut entries = Vec::new();
    let mut data = Vec::new();
    for (index, (stored, body)) in records.iter().enumerate() {
        let offset = start() + data.len() / PAYLOAD * (BLOCK + 16) + 5 + data.len() % PAYLOAD;
        let mut entry = json!({"index":index,"stored":stored});
        if !body.is_empty() {
            entry["body"] = json!({"offset":offset});
        }
        data.extend_from_slice(body);
        entries.push(entry);
    }
    edit(&mut entries);
    let mut blocks = Vec::new();
    put(&mut blocks, 0, &data);
    let offset = start() + blocks.len() * (BLOCK + 16) + 5;
    let index = page(1, &serde_json::to_vec(&entries).unwrap());
    put(&mut blocks, 0, &index);
    let footer = json!({"roots":[{"offset":offset,"bytes":index.len(),"first":0,
        "count":records.len(),"level":0}],"count":records.len(),"sealed":false,"block":blocks.len()});
    put(
        &mut blocks,
        3,
        &object(&serde_json::to_vec(&footer).unwrap()),
    );
    file(&blocks)
}
fn dictionary_file(data: &[u8]) -> Option<Vec<u8>> {
    let (&mode, bytes) = data.split_first()?;
    let base = if bytes.is_empty() {
        b"dictionary fixture".as_slice()
    } else {
        bytes
    };
    let mut changed = base.to_vec();
    changed[0] ^= 1;
    let base_object = packed(
        0,
        if mode % 5 == 3 { 2 } else { 0 },
        base.len() as u32,
        base,
    );
    let encoded = zstd::bulk::Compressor::with_dictionary(1, base)
        .ok()?
        .compress(&changed)
        .ok()?;
    let dependent = packed(0, 2, changed.len() as u32, &encoded);
    let stored = |bytes: &[u8], length: usize, boundary: &str| {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        json!({"revision":3,"sequence":1,"exchange_id":1,"kind":"body-segment",
            "payload":{"boundary":boundary,"byte_count":bytes.len(),"bytes":null,"truncated":false},
            "body_index":{"frame_bytes":length,"bytes":bytes.len(),"digest":digest}})
    };
    let records = [
        (
            stored(base, base_object.len(), "upstream-response"),
            base_object,
        ),
        (
            stored(&changed, dependent.len(), "client-response"),
            dependent,
        ),
    ];
    Some(records_with_refs(&records, |entries| {
        let mut descriptor = entries[0]["stored"]["body_index"].clone();
        if mode % 5 == 1 {
            descriptor["digest"][0] = (descriptor["digest"][0].as_u64().unwrap() ^ 1).into();
        }
        let offset = entries[usize::from(mode % 5 == 2)]["body"]["offset"].clone();
        entries[1]["body"]["dictionary"] = json!({"offset":offset,"descriptor":descriptor});
        if mode % 5 == 4 {
            entries[1]["stored"]["body_index"]["digest"][0] = (entries[1]["stored"]["body_index"]
                ["digest"][0]
                .as_u64()
                .unwrap()
                ^ 1)
            .into();
        }
    }))
}
fn connection_records(data: &[u8]) -> Option<Vec<u8>> {
    let mut records: Vec<Value> = serde_json::from_slice(data).ok()?;
    if records.len() > 32 {
        return None;
    }
    let mut identities = Vec::new();
    for stored in &mut records {
        if stored["kind"] == "connection-context" {
            let payload = stored.get_mut("payload")?.as_object_mut()?;
            let context = payload.get_mut("context")?;
            context.sort_all_objects();
            let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(context).ok()?));
            // Repair hashes so mutations reach definition validation and reference resolution.
            payload.insert("sha256".into(), digest.clone().into());
            identities.push(digest);
        }
    }
    for stored in &mut records {
        if stored["kind"] == "performance" {
            for sample in stored
                .get_mut("payload")?
                .get_mut("transports")?
                .as_array_mut()?
            {
                let sample = sample.as_object_mut()?;
                if let Some(index) = sample.get("context").and_then(Value::as_u64)
                    && let Some(digest) = identities.get(usize::try_from(index).ok()?)
                {
                    sample.insert("context".into(), digest.clone().into());
                }
            }
        }
    }
    Some(record_files(
        &records
            .into_iter()
            .map(|record| (record, Vec::new()))
            .collect::<Vec<_>>(),
    ))
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
                    bytes: Some((0..4096_u32).map(|i| (i * 31).to_le_bytes()[0]).collect()),
                    truncated: false,
                },
            })
            .unwrap();
        writer
            .append(&CaptureRecord {
                sequence: 2,
                exchange_id: 1,
                kind: CaptureRecordKind::Performance(
                    transmog_core::performance::PerformanceEvidence {
                        transports: vec![transmog_core::performance::TransportObservation {
                            leg: "upstream".into(),
                            connection_id: "fuzz-shared-connection".into(),
                            peer: Some("127.0.0.1:443".into()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                ),
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
    mutate_bytes(template(normal).to_vec(), data)
}
fn mutate_bytes(mut bytes: Vec<u8>, data: &[u8]) -> Vec<u8> {
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
fn tree_template() -> &'static [u8] {
    static TREE: OnceLock<Vec<u8>> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut blocks = Vec::new();
        let mut leaves = Vec::new();
        for index in 0..4 {
            let leaf = page(
                1,
                &serde_json::to_vec(&json!([{"index":index,"stored":{
                    "revision":3,"sequence":index,"exchange_id":1,"kind":"completed","payload":null
                }}]))
                .unwrap(),
            );
            leaves.push(json!({"offset":start()+blocks.len()*(BLOCK+16)+5,
                "bytes":leaf.len(),"first":index,"count":1,"level":0}));
            blocks.push((0, leaf));
        }
        let mut branches = Vec::new();
        for (index, children) in leaves.chunks(2).enumerate() {
            let branch = page(2, &serde_json::to_vec(children).unwrap());
            branches.push(json!({"offset":start()+blocks.len()*(BLOCK+16)+5,
                "bytes":branch.len(),"first":index*2,"count":2,"level":1}));
            blocks.push((0, branch));
        }
        let root = page(2, &serde_json::to_vec(&branches).unwrap());
        let node = json!({"offset":start()+blocks.len()*(BLOCK+16)+5,
            "bytes":root.len(),"first":0,"count":4,"level":2});
        blocks.push((0, root));
        let footer = json!({"roots":[node],"count":4,"sealed":false,"block":blocks.len()});
        blocks.push((3, object(&serde_json::to_vec(&footer).unwrap())));
        file(&blocks)
    })
}
pub(crate) fn limits(control: u8) -> CaptureLimits {
    CaptureLimits {
        max_file_bytes: 1024 * 1024,
        max_record_bytes: [128 * 1024, 16, 1024, 8192][(usize::from(control) >> 4) % 4],
        max_records: 256,
    }
}
fn exercise_file(bytes: &[u8], control: u8) {
    let limits = limits(control);
    let Ok(mut indexed) = CaptureReader::new(Cursor::new(bytes), limits) else {
        return;
    };
    let mut full = Some(CaptureReader::new(Cursor::new(bytes), limits).unwrap());
    let mut count = 0;
    let mut last_was_seal = false;
    loop {
        let expected = match indexed.read_next_indexed() {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                if let Some(full) = &mut full {
                    assert!(full.read_next().is_err());
                }
                return;
            }
        };
        assert!(count < limits.max_records);
        count += 1;
        last_was_seal = matches!(expected.record.kind, CaptureRecordKind::Seal { .. });
        let mut cursor = Cursor::new(bytes);
        cursor.set_position(expected.offset);
        let random = read_indexed_frame_with_codec(
            cursor,
            expected.frame_bytes,
            limits,
            &expected.codec,
            expected.index,
        );
        if let Some(full_reader) = &mut full {
            match (full_reader.read_next(), &random) {
                (Ok(Some(actual)), Ok(random)) => {
                    // DER is hydrated only by sequential readers. It is excluded
                    // from the wire record; compare every serialized field and body byte.
                    assert_eq!(actual.record.sequence, random.sequence);
                    assert_eq!(actual.record.exchange_id, random.exchange_id);
                    match (&actual.record.kind, &random.kind) {
                        (CaptureRecordKind::Performance(a), CaptureRecordKind::Performance(b)) => {
                            assert_eq!(
                                serde_json::to_value(a).unwrap(),
                                serde_json::to_value(b).unwrap()
                            );
                        }
                        (a, b) => assert_eq!(a, b),
                    }
                    assert_eq!(
                        actual.body.map(|body| (body.bytes, body.digest)),
                        expected.body.map(|body| (body.bytes, body.digest))
                    );
                    let mut record = actual.record;
                    if let CaptureRecordKind::BodySegment { bytes, .. } = &mut record.kind {
                        *bytes = None;
                    }
                    assert_eq!(record, expected.record);
                }
                (Err(_), Err(_)) => full = None, // Body integrity is checked lazily.
                other => panic!("sequential/random readers disagree: {other:?}"),
            }
        }
        if let Ok(record) = random
            && let Some(body) = expected.body
        {
            let CaptureRecordKind::BodySegment {
                bytes: Some(bytes), ..
            } = record.kind
            else {
                panic!("missing body")
            };
            assert_eq!(bytes.len(), body.bytes);
            assert_eq!(<[u8; 32]>::from(Sha256::digest(&bytes)), body.digest);
        }
    }
    if indexed.sealed() {
        assert!(last_was_seal);
    }
    if let Some(full) = &mut full {
        assert!(full.read_next().unwrap().is_none());
    }
}

#[allow(clippy::too_many_lines)] // Explicit bounded mutation modes match the semantic seed inventory.
pub(crate) fn fixture(input: &[u8]) -> Option<Vec<u8>> {
    let (&control, data) = input.split_first()?;
    if data.len() > 65536 {
        return None;
    }
    let bytes = match control & 15 {
        0 => data.to_vec(),
        1 | 10 => mutate_template(data, control & 15 == 10),
        2 => file(&[(3, object(&data[..data.len().min(PAYLOAD - 5)]))]),
        3 => {
            let Ok(stored) = serde_json::from_slice::<Value>(data) else {
                return None;
            };
            record_file(stored, &[])
        }
        4 => {
            let mut children: Value = serde_json::from_slice(data).ok()?;
            for child in children.as_array_mut()? {
                if let Some(offset) = child["offset"].as_u64() {
                    child["offset"] = offset.saturating_add(start() as u64).into();
                }
            }
            let raw = serde_json::to_vec(&children).ok()?;
            let encoded = zstd::bulk::compress(&raw, 1).ok()?;
            let bytes = packed(2, 1, raw.len() as u32, &encoded);
            if bytes.len() > PAYLOAD {
                return None;
            }
            let footer = json!({"roots":[{"offset":start()+BLOCK+16+5,"bytes":bytes.len(),
                "first":0,"count":1,"level":1}],"count":1,"sealed":false,"block":2});
            file(&[
                (0, page(1, b"[]")),
                (0, bytes),
                (3, object(&serde_json::to_vec(&footer).unwrap())),
            ])
        }
        5 | 6 => {
            if data.len() < 8 {
                return None;
            }
            let body = &data[8..];
            let mut encoded = zstd::bulk::compress(body, 1).unwrap();
            if !encoded.is_empty() {
                let p = usize::from(data[0]) % encoded.len();
                encoded[p] ^= data[1];
            }
            let expanded = if control & 15 == 5 {
                body.len() as u32
            } else {
                u32::from_le_bytes(data[2..6].try_into().unwrap())
            };
            let encoded = packed(data[6], data[7], expanded, &encoded);
            let digest: [u8; 32] = Sha256::digest(body).into();
            let stored = json!({"revision":3,"sequence":1,"exchange_id":1,"kind":"body-segment",
                "payload":{"boundary":"client-response","byte_count":body.len(),"bytes":null,"truncated":false},
                "body_index":{"frame_bytes":encoded.len(),"bytes":body.len(),"digest":digest}});
            record_file(stored, &encoded)
        }
        8 => {
            let digest = format!("{:x}", Sha256::digest(data));
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
                return None;
            };
            let start = 13 + header().len() + 4 + 512;
            for entry in &mut entries {
                if let Some(offset) = entry["body"]["offset"].as_u64() {
                    entry["body"]["offset"] = offset.saturating_add(start as u64).into();
                }
            }
            let index = page(1, &serde_json::to_vec(&entries).unwrap());
            let mut blocks = vec![(0, vec![0; PAYLOAD])];
            put(&mut blocks, 0, &index);
            let footer = json!({"roots":[{"offset":start+BLOCK+16+5,"bytes":index.len(),
                "first":0,"count":entries.len(),"level":0}],"count":entries.len(),"sealed":false,"block":blocks.len()});
            put(
                &mut blocks,
                3,
                &object(&serde_json::to_vec(&footer).unwrap()),
            );
            file(&blocks)
        }
        12 => connection_records(data)?,
        13 => dictionary_file(data)?,
        15 => mutate_bytes(tree_template().to_vec(), data),
        14 => {
            // Keep hostile stored JSON raw until the production reader applies
            // its record limit and nesting checks.
            let (&count, raw) = data.split_first()?;
            let page = page(1, raw);
            if page.len() > PAYLOAD {
                return None;
            }
            let count = u64::from(count) + 1;
            let footer = json!({"roots":[{"offset":start()+5,"bytes":page.len(),
                "first":0,"count":count,"level":0}],"count":count,"sealed":false,"block":1});
            file(&[
                (0, page),
                (3, object(&serde_json::to_vec(&footer).unwrap())),
            ])
        }
        _ => {
            // Raw compressed index object: correct envelope, hostile codec/length/zstd bytes.
            let data = &data[..data.len().min(PAYLOAD)];
            let footer = json!({"roots":[{"offset":start()+5,"bytes":data.len(),"first":0,
                "count":1,"level":0}],"count":1,"sealed":false,"block":1});
            file(&[
                (0, data.to_vec()),
                (3, object(&serde_json::to_vec(&footer).unwrap())),
            ])
        }
    };
    Some(bytes)
}

pub(crate) fn exercise(input: &[u8]) {
    if let Some(bytes) = fixture(input) {
        exercise_file(&bytes, input[0]);
    }
}
