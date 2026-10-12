use super::*;
use std::fmt::Write as _;
use std::io::Cursor;
use transmog_core::observe::ExchangeBoundary;

fn pair(id: u128, upstream: bool, bytes: Vec<u8>) -> CaptureRecord {
    let mut record = super::tests::body(id, bytes);
    if upstream && let CaptureRecordKind::BodySegment { boundary, .. } = &mut record.kind {
        *boundary = ExchangeBoundary::UpstreamResponse;
    }
    record
}

#[test]
fn body_expansion_must_match_index_before_decoding() {
    for password in [None, Some(CapturePassword::new("body size review".into()))] {
        let limits = CaptureLimits::default();
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            limits,
            &CaptureEncoding {
                password: password.clone(),
            },
        )
        .unwrap();
        let mut http = super::tests::body(1, vec![0; 8]);
        if let CaptureRecordKind::BodySegment { bytes, .. } = &mut http.kind {
            *bytes = None;
        }
        let stream = CaptureRecord {
            exchange_id: 2,
            sequence: 1,
            kind: CaptureRecordKind::StreamMessage(transmog_core::stream::StreamMessage::new(
                "server",
                "event",
                Vec::new().into(),
            )),
        };
        for record in [http, stream] {
            let mut stored = store(&record).unwrap();
            stored.body_index = Some(StoredBodyIndex {
                frame_bytes: 0,
                bytes: 8,
                digest: Sha256::digest([0; 8]).into(),
            });
            // Intentionally bypass normal compression to produce conflicting sizes.
            // Invalid zstd proves rejection happens before invoking its decoder:
            // decoding first would return an I/O error instead of InvalidMagic.
            writer.jobs.push_back(PendingRecord {
                stored,
                index: writer.records,
                body: Some(PendingBody::Ready(CompressedChunk {
                    codec: 1,
                    expanded: 32 * 1024,
                    bytes: b"invalid zstd".to_vec(),
                })),
                raw: Some(vec![0; 8]),
            });
            writer.records += 1;
            writer.dirty = true;
            writer.finish_one().unwrap();
        }
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap();
        let mut reader =
            CaptureReader::with_password(Cursor::new(&bytes), limits, password.as_ref()).unwrap();
        let mut full =
            CaptureReader::with_password(Cursor::new(&bytes), limits, password.as_ref()).unwrap();
        for _ in 0..2 {
            let frame = reader.read_next_indexed().unwrap().unwrap();
            assert!(matches!(full.read_next(), Err(CaptureError::InvalidMagic)));
            let mut input = Cursor::new(&bytes);
            input.set_position(frame.offset);
            assert!(matches!(
                read_indexed_frame_with_codec(
                    input,
                    frame.frame_bytes,
                    limits,
                    &frame.codec,
                    frame.index,
                ),
                Err(CaptureError::InvalidMagic)
            ));
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One fixture matrix exercises plain/encrypted checkpoint/compaction paths.
fn exact_and_near_identical_boundaries_share_bodies_with_bounded_lazy_reads() {
    for encrypted in [false, true] {
        for checkpoint in [false, true] {
            let options = CaptureEncoding {
                password: encrypted.then(|| CapturePassword::new("shared-body-test".into())),
            };
            let mut writer =
                CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options)
                    .unwrap();
            let original = super::tests::binary();
            let mut changed = original.clone();
            changed[60_000..60_100].fill(42);
            let expected = [
                pair(1, true, original.clone()),
                pair(1, false, changed),
                pair(1, false, original),
            ];
            writer.append(&expected[0]).unwrap();
            if checkpoint {
                writer.checkpoint().unwrap();
            }
            writer.append(&expected[1]).unwrap();
            writer.positions.remove(&(1, "client-response".into()));
            writer.append(&expected[2]).unwrap();
            writer.seal().unwrap();
            let compact = writer.compact_sealed_bytes().unwrap();
            let bytes = writer.into_inner().unwrap();
            for bytes in std::iter::once(bytes).chain(compact) {
                assert!(
                    bytes.len() < BODY_CHUNK_BYTES + 80_000,
                    "{} bytes",
                    bytes.len()
                );
                let mut reader = CaptureReader::with_password(
                    super::tests::Counted {
                        cursor: Cursor::new(bytes.clone()),
                        read: 0,
                    },
                    CaptureLimits::default(),
                    options.password.as_ref(),
                )
                .unwrap();
                let mut frames = Vec::new();
                while let Some(frame) = reader.read_next_indexed().unwrap() {
                    frames.push(frame);
                }
                assert_eq!(frames[0].offset, frames[2].offset);
                assert_ne!(frames[0].offset, frames[1].offset);
                assert!(frames[1].frame_bytes < 1000);
                assert!(
                    reader.input.read < 96 * 1024,
                    "metadata-only reads: {}",
                    reader.input.read
                );
                let location = frames[1].codec.record.as_ref().unwrap();
                let json = packed::read_object(
                    &mut Cursor::new(&bytes),
                    &frames[1].codec,
                    location.page.offset,
                    u64::from(location.page.bytes),
                    LEAF,
                    INDEX_LIMIT,
                    None,
                    None,
                )
                .unwrap();
                let entries: Vec<Entry> = serde_json::from_slice(&json).unwrap();
                let entry = entries.iter().find(|entry| entry.index == 1).unwrap();
                let base = entry.body.as_ref().unwrap().dictionary.as_ref().unwrap();
                assert_eq!(base.offset, frames[0].offset);
                for i in [1, 2, 0] {
                    let frame = &frames[i];
                    let mut source = super::tests::Counted {
                        cursor: Cursor::new(bytes.clone()),
                        read: 0,
                    };
                    source.cursor.set_position(frame.offset);
                    assert_eq!(
                        read_indexed_frame_with_codec(
                            &mut source,
                            frame.frame_bytes,
                            CaptureLimits::default(),
                            &frame.codec,
                            frame.index
                        )
                        .unwrap(),
                        expected[i]
                    );
                    assert!(
                        source.read < BODY_CHUNK_BYTES + 96 * 1024,
                        "lazy read {}",
                        source.read
                    );
                }
                // A damaged independent dictionary must fail when reading its dependent body.
                let mut damaged = bytes.clone();
                damaged[usize::try_from(base.offset).unwrap() + 40_000] ^= 1;
                let frame = &frames[1];
                let mut source = Cursor::new(damaged);
                source.set_position(frame.offset);
                assert!(
                    read_indexed_frame_with_codec(
                        source,
                        frame.frame_bytes,
                        CaptureLimits::default(),
                        &frame.codec,
                        frame.index
                    )
                    .is_err()
                );
            }
        }
    }
}

#[test]
fn dictionary_digest_and_single_hop_rule_are_enforced() {
    for scenario in 0..3 {
        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        writer.small = None;
        let original = super::tests::binary();
        let mut changed = original.clone();
        changed[20_000] ^= 1;
        writer.append(&pair(1, true, original)).unwrap();
        writer.append(&pair(1, false, changed.clone())).unwrap();
        let dependent = writer.entries[1].clone();
        if scenario == 2 {
            // Point another dependency at a codec-2 blob: it is backward, but
            // dictionary chaining is forbidden even when both digests are valid.
            changed[30_000] ^= 1;
            writer.append(&pair(3, false, changed)).unwrap();
            writer.entries[2].body.as_mut().unwrap().dictionary = Some(packed::BaseRef {
                offset: dependent.body.as_ref().unwrap().offset,
                descriptor: dependent.stored.body_index.unwrap(),
            });
        } else {
            let base = writer.entries[1]
                .body
                .as_mut()
                .unwrap()
                .dictionary
                .as_mut()
                .unwrap();
            if scenario == 0 {
                base.descriptor.digest[0] ^= 1;
            } else {
                base.descriptor.bytes -= 1;
            }
        }
        let bytes = writer.into_inner().unwrap();
        let mut indexed =
            CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        while indexed.read_next_indexed().unwrap().is_some() {}
        let mut full = CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        full.read_next().unwrap().unwrap();
        if scenario == 2 {
            full.read_next().unwrap().unwrap();
        }
        assert!(full.read_next().is_err(), "scenario {scenario}");
    }
}

#[test]
fn similar_headers_compress_together_without_duplicate_metadata_records() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    let value = super::tests::binary()[..32_000]
        .iter()
        .fold(String::new(), |mut value, byte| {
            write!(&mut value, "{byte:02x}").unwrap();
            value
        });
    for upstream in [false, true] {
        writer
            .append(&CaptureRecord {
                sequence: 1,
                exchange_id: 1,
                kind: CaptureRecordKind::Unknown {
                    kind: "header-compression-control".into(),
                    payload: serde_json::json!({"headers":value, "upstream":upstream}),
                },
            })
            .unwrap();
    }
    writer.seal().unwrap();
    let bytes = writer.into_inner().unwrap();
    assert!(
        bytes.len() < 40_000,
        "paired metadata occupies {} bytes",
        bytes.len()
    );
    let mut reader = CaptureReader::new(Cursor::new(bytes), CaptureLimits::default()).unwrap();
    assert!(reader.read_next_indexed().unwrap().is_some());
    assert!(reader.read_next_indexed().unwrap().is_some());
}

// Local forensic fixture, intentionally excluded from source control. Accepts
// StoredRecord JSONL with inline bytes and shared connection definitions.
#[test]
#[ignore = "set TMCAP_RECORDS and TMCAP_REPACKED to local forensic paths"]
fn repack_local_records_and_measure_metadata_open() {
    let records = std::fs::read_to_string(std::env::var("TMCAP_RECORDS").unwrap()).unwrap();
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    let mut contexts = crate::connections::ReaderContexts::default();
    let mut expected_bodies = Vec::new();
    let mut expected_records = Vec::new();
    let mut count = 0;
    for line in records.lines() {
        let stored: StoredRecord = serde_json::from_str(line).unwrap();
        if stored.kind == "seal" {
            continue;
        }
        let codec = contexts.prepare(&stored, &writer.codec).unwrap();
        let record = if stored.kind == "performance" {
            stored.load_container_metadata(&codec).unwrap().0
        } else {
            super::super::load(stored).unwrap()
        };
        if let CaptureRecordKind::BodySegment {
            bytes: Some(bytes), ..
        } = &record.kind
        {
            expected_bodies.push((
                record.exchange_id,
                record.sequence,
                <[u8; 32]>::from(Sha256::digest(bytes)),
            ));
        }
        expected_records.push(serde_json::to_value(store(&record).unwrap()).unwrap());
        writer.append(&record).unwrap();
        count += 1;
    }
    writer.seal().unwrap();
    let bytes = writer.into_inner().unwrap();
    let mut reader = CaptureReader::new(
        super::tests::Counted {
            cursor: Cursor::new(bytes.clone()),
            read: 0,
        },
        CaptureLimits::default(),
    )
    .unwrap();
    let opening_reads = reader.input.read;
    let start = std::time::Instant::now();
    let mut indexed = 0;
    while reader.read_next_indexed().unwrap().is_some() {
        indexed += 1;
    }
    println!(
        "repacked: bytes={} records={indexed} open_read={opening_reads} index_read={} index_ms={:.2}",
        bytes.len(),
        reader.input.read,
        start.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(indexed, count + 1);
    let mut reader = CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
    let mut actual = Vec::new();
    let mut record_index = 0;
    while let Some(frame) = reader.read_next().unwrap() {
        if !matches!(frame.record.kind, CaptureRecordKind::Seal { .. }) {
            let value = serde_json::to_value(store(&frame.record).unwrap()).unwrap();
            assert!(
                value == expected_records[record_index],
                "record {record_index} changed"
            );
            record_index += 1;
        }
        if let CaptureRecordKind::BodySegment {
            bytes: Some(bytes), ..
        } = frame.record.kind
        {
            actual.push((
                frame.record.exchange_id,
                frame.record.sequence,
                <[u8; 32]>::from(Sha256::digest(&bytes)),
            ));
        }
    }
    assert_eq!(actual, expected_bodies);
    std::fs::write(std::env::var("TMCAP_REPACKED").unwrap(), bytes).unwrap();
}

#[test]
#[ignore = "synthetic large-trace I/O benchmark; run in release mode"]
fn large_trace_metadata_io_measurement() {
    let source = super::tests::binary();
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    for id in 0..2000_u128 {
        let mut bytes = source[..32 * 1024].to_vec();
        bytes[..16].copy_from_slice(&id.to_le_bytes());
        writer.append(&pair(id, true, bytes.clone())).unwrap();
        writer.append(&pair(id, false, bytes)).unwrap();
    }
    writer.seal().unwrap();
    let bytes = writer.into_inner().unwrap();
    let size = bytes.len();
    let started = std::time::Instant::now();
    let mut reader = CaptureReader::new(
        super::tests::Counted {
            cursor: Cursor::new(bytes),
            read: 0,
        },
        CaptureLimits::default(),
    )
    .unwrap();
    let open_bytes = reader.input.read;
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let indexed = std::time::Instant::now();
    let mut count = 0;
    let mut chosen = None;
    while let Some(frame) = reader.read_next_indexed().unwrap() {
        count += 1;
        if frame.record.exchange_id == 1234 {
            chosen = Some(frame);
        }
    }
    let index_ms = indexed.elapsed().as_secs_f64() * 1000.0;
    let index_bytes = reader.input.read;
    assert_eq!(count, 4001);
    assert!(index_bytes < size / 20);
    let frame = chosen.unwrap();
    reader.input.cursor.set_position(frame.offset);
    let started = std::time::Instant::now();
    let _ = read_indexed_frame_with_codec(
        &mut reader.input,
        frame.frame_bytes,
        CaptureLimits::default(),
        &frame.codec,
        frame.index,
    )
    .unwrap();
    let body_ms = started.elapsed().as_secs_f64() * 1000.0;
    let body_bytes = reader.input.read - index_bytes;
    assert!(body_bytes < 128 * 1024);
    println!(
        "large trace: file_bytes={size} records={count} open_bytes={open_bytes} open_ms={open_ms:.3} index_bytes={index_bytes} index_ms={index_ms:.3} selected_body_read_bytes={body_bytes} body_ms={body_ms:.3}"
    );
}
