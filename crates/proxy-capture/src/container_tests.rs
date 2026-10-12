use super::*;
use std::io::Cursor;
use transmog_core::observe::ExchangeBoundary;

pub(super) struct Counted {
    pub(super) cursor: Cursor<Vec<u8>>,
    pub(super) read: usize,
}
impl Read for Counted {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.cursor.read(bytes)?;
        self.read += count;
        Ok(count)
    }
}
impl Seek for Counted {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(position)
    }
}
pub(super) fn body(id: u128, bytes: Vec<u8>) -> CaptureRecord {
    CaptureRecord {
        sequence: 1,
        exchange_id: id,
        kind: CaptureRecordKind::BodySegment {
            boundary: ExchangeBoundary::ClientResponse,
            byte_count: bytes.len(),
            bytes: Some(bytes),
            truncated: false,
        },
    }
}
pub(super) fn binary() -> Vec<u8> {
    let mut seed = 0x0123_4567_89ab_cdef_u64;
    (0..BODY_CHUNK_BYTES)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed.to_le_bytes()[0]
        })
        .collect()
}
#[test]
fn opening_and_indexing_do_not_read_bodies_and_random_seek_authenticates_only_selected_chunk() {
    for password in [None, Some(CapturePassword::new("index secret".into()))] {
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding {
                password: password.clone(),
            },
        )
        .unwrap();
        let source = binary();
        for id in 0..128_u128 {
            let mut unique = source.clone();
            unique[..16].copy_from_slice(&id.to_le_bytes());
            writer.append(&body(id, unique)).unwrap();
        }
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap();
        let size = bytes.len();
        let mut reader = CaptureReader::with_password(
            Counted {
                cursor: Cursor::new(bytes.clone()),
                read: 0,
            },
            CaptureLimits::default(),
            password.as_ref(),
        )
        .unwrap();
        assert!(
            reader.input.read < 32 * 1024,
            "opening read {} bytes",
            reader.input.read
        );
        let mut selected = None;
        while let Some(frame) = reader.read_next_indexed().unwrap() {
            if frame.record.exchange_id == 100 {
                selected = Some(frame);
            }
        }
        assert!(
            reader.input.read < size / 20,
            "index read {} of {size} bytes",
            reader.input.read
        );
        let frame = selected.unwrap();
        let mut cursor = Cursor::new(bytes.clone());
        cursor.set_position(frame.offset);
        assert_eq!(
            read_indexed_frame_with_codec(
                &mut cursor,
                frame.frame_bytes,
                CaptureLimits::default(),
                &frame.codec,
                frame.index
            )
            .unwrap(),
            {
                let mut expected = source;
                expected[..16].copy_from_slice(&100_u128.to_le_bytes());
                body(100, expected)
            }
        );
        let mut changed = bytes;
        let block =
            usize::try_from((frame.offset - frame.codec.container_start()) / BLOCK_BYTES as u64)
                .unwrap()
                + 1;
        let changed_byte =
            usize::try_from(frame.codec.container_start()).unwrap() + block * BLOCK_BYTES + 70;
        changed[changed_byte] ^= 1;
        let mut indexed = CaptureReader::with_password(
            Cursor::new(&changed),
            CaptureLimits::default(),
            password.as_ref(),
        )
        .unwrap();
        while indexed.read_next_indexed().unwrap().is_some() {}
        let mut cursor = Cursor::new(changed);
        cursor.set_position(frame.offset);
        assert!(
            read_indexed_frame_with_codec(
                cursor,
                frame.frame_bytes,
                CaptureLimits::default(),
                &frame.codec,
                frame.index
            )
            .is_err()
        );
    }
}
#[test]
fn encrypted_blocks_hide_record_types_lengths_flags_and_index_contents() {
    let options = CaptureEncoding {
        password: Some(CapturePassword::new("private".into())),
    };
    let mut writer =
        CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options).unwrap();
    for id in 0..10 {
        writer.append(&CaptureRecord { sequence: 1, exchange_id: id, kind: CaptureRecordKind::Unknown { kind: "sensitive-record-kind".into(), payload: serde_json::json!({"target":"https://secret.example/private", "timestamp":12345}) } }).unwrap();
    }
    writer.seal().unwrap();
    let bytes = writer.into_inner().unwrap();
    for marker in [
        b"sensitive-record-kind".as_slice(),
        b"secret.example",
        b"exchange_id",
        b"frame_bytes",
    ] {
        assert!(!bytes.windows(marker.len()).any(|window| window == marker));
    }
    let reader = CaptureReader::with_password(
        Cursor::new(&bytes),
        CaptureLimits::default(),
        options.password.as_ref(),
    )
    .unwrap();
    assert_eq!(
        (bytes.len() - usize::try_from(reader.codec.container_start()).unwrap())
            % reader.codec.block_bytes(),
        0
    );
    assert!(matches!(
        CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()),
        Err(CaptureError::PasswordRequired)
    ));
    assert!(matches!(
        CaptureReader::with_password(
            Cursor::new(&bytes),
            CaptureLimits::default(),
            Some(&CapturePassword::new("wrong".into()))
        ),
        Err(CaptureError::InvalidPassword)
    ));
}
#[test]
fn alternating_checkpoint_locators_survive_torn_update_and_find_a_bounded_prefix() {
    let options = CaptureEncoding {
        password: Some(CapturePassword::new("checkpoint".into())),
    };
    let mut writer =
        CaptureWriter::with_encoding(Cursor::new(Vec::new()), CaptureLimits::default(), &options)
            .unwrap();
    writer.enable_checkpoint_locators();
    writer.append(&body(1, binary())).unwrap();
    writer.checkpoint().unwrap();
    let first = writer.bytes_written();
    writer.append(&body(2, binary())).unwrap();
    writer.checkpoint().unwrap();
    let mut bytes = writer.into_inner().unwrap().into_inner();
    // Destroy the newest locator and its footer as if the process died in
    // publication; the old authenticated locator remains usable.
    let header = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize + 13 + 20;
    bytes[header..header + 80].fill(0);
    bytes.truncate(bytes.len() - 100);
    let reader = CaptureReader::with_password(
        Counted {
            cursor: Cursor::new(bytes),
            read: 0,
        },
        CaptureLimits::default(),
        options.password.as_ref(),
    )
    .unwrap();
    assert_eq!(reader.valid_bytes(), first);
    assert!(!reader.sealed());
    assert!(reader.truncated_tail());
    assert!(reader.input.read < 64 * 1024);
}
#[test]
fn large_bodies_split_without_losing_observed_or_retained_bytes() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    let expected = binary().repeat(7);
    writer.append(&body(1, expected.clone())).unwrap();
    writer.seal().unwrap();
    let mut reader = CaptureReader::new(
        Cursor::new(writer.into_inner().unwrap()),
        CaptureLimits::default(),
    )
    .unwrap();
    let mut actual = Vec::new();
    let mut observed = 0;
    while let Some(frame) = reader.read_next().unwrap() {
        if let CaptureRecordKind::BodySegment {
            byte_count,
            bytes: Some(bytes),
            ..
        } = frame.record.kind
        {
            assert!(bytes.len() <= BODY_CHUNK_BYTES);
            actual.extend(bytes);
            observed += byte_count;
        }
    }
    assert_eq!(actual, expected);
    assert_eq!(observed, expected.len());
    assert!(reader.sealed());
}
#[test]
fn metadata_tree_spills_and_preserves_order_with_bounded_leaf_pages() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    for id in 0..10000 {
        writer
            .append(&CaptureRecord {
                sequence: u64::try_from(id).unwrap() + 1,
                exchange_id: id,
                kind: CaptureRecordKind::Unknown {
                    kind: "fixture".into(),
                    payload: serde_json::json!({"text":"x".repeat(1024)}),
                },
            })
            .unwrap();
    }
    writer.seal().unwrap();
    let mut reader = CaptureReader::new(
        Cursor::new(writer.into_inner().unwrap()),
        CaptureLimits::default(),
    )
    .unwrap();
    for id in 0..10000 {
        assert_eq!(
            reader
                .read_next_indexed()
                .unwrap()
                .unwrap()
                .record
                .exchange_id,
            id
        );
    }
    assert!(matches!(
        reader.read_next_indexed().unwrap().unwrap().record.kind,
        CaptureRecordKind::Seal { .. }
    ));
    assert!(reader.read_next_indexed().unwrap().is_none());
}

#[test]
fn pool_preserves_order_and_checkpoints_drain_outstanding_work() {
    let mut writer = CaptureWriter::new(Cursor::new(Vec::new()), CaptureLimits::default()).unwrap();
    writer.set_compression_workers(2).unwrap();
    writer.enable_checkpoint_locators();
    let mut expected = Vec::new();
    for i in 0..25 {
        let mut record = body(i % 2, vec![b'a' + (i % 20) as u8; BODY_CHUNK_BYTES]);
        record.sequence = u64::try_from(i).unwrap() + 1;
        writer.append(&record).unwrap();
        expected.push(record);
    }
    assert!(writer.pool.is_some());
    assert!(writer.jobs.len() <= 8);
    writer.checkpoint().unwrap();
    assert!(writer.jobs.is_empty());
    let committed = writer.output.get_ref().clone();
    let mut reader = CaptureReader::new(Cursor::new(committed), CaptureLimits::default()).unwrap();
    for record in &expected {
        assert_eq!(&reader.read_next().unwrap().unwrap().record, record);
    }
    assert!(reader.read_next().unwrap().is_none());
    writer.seal().unwrap();
    let mut reader = CaptureReader::new(
        Cursor::new(writer.into_inner().unwrap().into_inner()),
        CaptureLimits::default(),
    )
    .unwrap();
    for record in expected {
        assert_eq!(reader.read_next().unwrap().unwrap().record, record);
    }
    assert!(matches!(
        reader.read_next().unwrap().unwrap().record.kind,
        CaptureRecordKind::Seal { record_count: 25 }
    ));
}

#[test]
fn incompressible_stream_never_starts_pool_and_bursts_use_large_os_writes() {
    #[derive(Default)]
    struct Writes {
        sizes: Vec<usize>,
        bytes: u64,
    }
    impl Write for Writes {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.sizes.push(bytes.len());
            self.bytes += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = CaptureWriter::new(Writes::default(), CaptureLimits::default()).unwrap();
    writer.set_compression_workers(2).unwrap();
    let bootstrap_calls = writer.output.sizes.len();
    for _ in 0..256 {
        writer.append(&body(1, binary())).unwrap();
    }
    assert!(writer.pool.is_none());
    assert!(writer.compressor.attempted_bytes() < 1024 * 1024);
    writer.checkpoint().unwrap();
    let batches = &writer.output.sizes[bootstrap_calls + 1..];
    assert!(batches.len() < 40);
    assert!(
        batches[..batches.len() - 1]
            .iter()
            .all(|size| *size >= 1024 * 1024)
    );
    assert_eq!(writer.output.bytes, writer.bytes_written());
}

#[test]
fn short_writes_roundtrip_and_write_failure_poisoning_prevents_nonce_reuse() {
    struct Short {
        bytes: Vec<u8>,
        fail: bool,
    }
    impl Write for Short {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail {
                return Err(io::Error::other("injected write failure"));
            }
            let count = bytes.len().min(7);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = CaptureWriter::new(
        Short {
            bytes: Vec::new(),
            fail: false,
        },
        CaptureLimits::default(),
    )
    .unwrap();
    let record = body(7, binary());
    writer.append(&record).unwrap();
    writer.checkpoint().unwrap();
    let mut reader =
        CaptureReader::new(Cursor::new(&writer.output.bytes), CaptureLimits::default()).unwrap();
    assert_eq!(reader.read_next().unwrap().unwrap().record, record);
    writer.append(&record).unwrap();
    writer.output.fail = true;
    assert!(writer.checkpoint().is_err());
    assert!(matches!(
        writer.append(&record),
        Err(CaptureError::AlreadySealed)
    ));
    assert!(writer.seal().is_err());
}

#[test]
fn encrypted_block_authentication_binds_ordinal_file_and_key_epoch() {
    let options = CaptureEncoding {
        password: Some(CapturePassword::new("epoch test".into())),
    };
    let first =
        CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options).unwrap();
    let second =
        CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options).unwrap();
    let bytes = vec![42; BLOCK_PLAIN];
    for ordinal in [0, 65_535, 65_536, 131_072] {
        let encoded = first.codec.encode_block(ordinal, &bytes).unwrap();
        assert_eq!(first.codec.decode_block(ordinal, &encoded).unwrap(), bytes);
        assert!(first.codec.decode_block(ordinal + 1, &encoded).is_err());
        assert!(second.codec.decode_block(ordinal, &encoded).is_err());
    }
    let locator = first.codec.encode_locator(1, 0, 0).unwrap();
    assert_eq!(first.codec.decode_locator(&locator, 0).unwrap(), (1, 0));
    assert!(first.codec.decode_locator(&locator, 1).is_err());
    assert_ne!(locator, first.codec.encode_locator(1, 0, 0).unwrap());
}

#[test]
fn index_respects_caller_metadata_bounds() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    writer.append(&body(1, vec![0; 100])).unwrap();
    writer.seal().unwrap();
    let mut reader = CaptureReader::new(
        Cursor::new(writer.into_inner().unwrap()),
        CaptureLimits {
            max_record_bytes: 32,
            ..CaptureLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        reader.read_next_indexed(),
        Err(CaptureError::RecordTooLarge { .. })
    ));
}

#[test]
fn small_captures_keep_the_block_structure_with_about_five_kib_overhead() {
    for password in [None, Some(CapturePassword::new("small secret".into()))] {
        for length in [0, 128, 4096, 16_384, 65_536] {
            let mut writer = CaptureWriter::with_encoding(
                Vec::new(),
                CaptureLimits::default(),
                &CaptureEncoding {
                    password: password.clone(),
                },
            )
            .unwrap();
            let expected = body(1, binary()[..length].to_vec());
            if length != 0 {
                writer.append(&expected).unwrap();
            }
            writer.seal().unwrap();
            let bytes = writer.into_inner().unwrap();
            assert!(
                bytes.len() <= length + 5 * 1024,
                "length={length}, file={}",
                bytes.len()
            );
            let mut reader = CaptureReader::with_password(
                Counted {
                    cursor: Cursor::new(bytes),
                    read: 0,
                },
                CaptureLimits::default(),
                password.as_ref(),
            )
            .unwrap();
            assert_eq!(reader.codec.block_plain(), 1024);
            assert!(reader.input.read < 2048);
            if length != 0 {
                assert_eq!(reader.read_next().unwrap().unwrap().record, expected);
            }
            assert!(matches!(
                reader.read_next().unwrap().unwrap().record.kind,
                CaptureRecordKind::Seal { .. }
            ));
            assert!(reader.read_next().unwrap().is_none());
        }
    }
}

#[test]
fn compacting_a_checkpointed_file_uses_fresh_encryption_and_preserves_indexed_reads() {
    let options = CaptureEncoding {
        password: Some(CapturePassword::new("replace safely".into())),
    };
    let mut writer =
        CaptureWriter::with_encoding(Cursor::new(Vec::new()), CaptureLimits::default(), &options)
            .unwrap();
    writer.enable_checkpoint_locators();
    writer.checkpoint().unwrap();
    let expected = body(1, binary()[..4096].to_vec());
    writer.append(&expected).unwrap();
    writer.checkpoint().unwrap();
    writer.seal().unwrap();
    let before = writer.output.get_ref().clone();
    let compact = writer.compact_sealed_bytes().unwrap().unwrap();
    assert!(compact.len() < 4096 + 5 * 1024);
    assert!(compact.len() < before.len());
    let mut reader = CaptureReader::with_password(
        Cursor::new(&compact),
        CaptureLimits::default(),
        options.password.as_ref(),
    )
    .unwrap();
    assert_eq!(reader.read_next().unwrap().unwrap().record, expected);
    let header = |bytes: &[u8]| {
        let length = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
        serde_json::from_slice::<serde_json::Value>(&bytes[13..13 + length]).unwrap()
    };
    assert_ne!(header(&before)["salt"], header(&compact)["salt"]);
    assert_eq!(writer.output.get_ref(), &before);
}

#[test]
fn writer_rejects_metadata_that_cannot_fit_a_readable_index_object() {
    let mut writer = CaptureWriter::new(
        Vec::new(),
        CaptureLimits {
            max_record_bytes: 32 * 1024 * 1024,
            ..CaptureLimits::default()
        },
    )
    .unwrap();
    let record = CaptureRecord {
        sequence: 1,
        exchange_id: 1,
        kind: CaptureRecordKind::Unknown {
            kind: "large-metadata".into(),
            payload: serde_json::Value::String("x".repeat(INDEX_LIMIT)),
        },
    };
    assert!(matches!(
        writer.append(&record),
        Err(CaptureError::RecordTooLarge { .. })
    ));
    assert!(writer.seal().is_err());
}

#[test]
fn stream_payload_is_lazy_authenticated_and_round_trips() {
    use transmog_core::stream::StreamMessage;
    let mut message = StreamMessage::new("server", "event", vec![b'x'; 24 * 1024 * 1024].into());
    message.raw = Some("data: x\n\n".into());
    message.event_id = Some("42".into());
    let record = CaptureRecord {
        exchange_id: 7,
        sequence: 9,
        kind: CaptureRecordKind::StreamMessage(message.clone()),
    };
    let mut writer = CaptureWriter::new(Cursor::new(Vec::new()), CaptureLimits::default()).unwrap();
    writer.append(&record).unwrap();
    writer.seal().unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();
    let mut reader =
        CaptureReader::new(Cursor::new(bytes.clone()), CaptureLimits::default()).unwrap();
    let frame = reader.read_next_indexed().unwrap().unwrap();
    let CaptureRecordKind::StreamMessage(metadata) = frame.record.kind else {
        panic!("message")
    };
    assert!(metadata.payload.is_empty());
    assert!(metadata.raw.is_none());
    assert!(frame.body.is_some());
    let mut source = Cursor::new(bytes.clone());
    source.set_position(frame.offset);
    let loaded = super::super::read_indexed_frame_with_codec(
        &mut source,
        frame.frame_bytes,
        CaptureLimits::default(),
        &frame.codec,
        frame.index,
    )
    .unwrap();
    assert_eq!(loaded, record);
    let mut reader = CaptureReader::new(Cursor::new(bytes), CaptureLimits::default()).unwrap();
    assert_eq!(reader.read_next().unwrap().unwrap().record, record);
}

#[test]
fn persistent_sse_ids_grow_once_and_remain_shared_after_random_reads() {
    use std::fmt::Write as _;
    use std::sync::Arc;
    use transmog_core::stream::StreamMessage;
    let mut id = String::new();
    for byte in &binary()[..32 * 1024] {
        write!(&mut id, "{byte:02x}").unwrap();
    }
    let id: Arc<str> = id.into();
    let mut sizes = Vec::new();
    for count in [128, 1024] {
        let mut writer =
            CaptureWriter::new(Cursor::new(Vec::new()), CaptureLimits::default()).unwrap();
        for sequence in 1..=count {
            let mut message = StreamMessage::new("server", "heartbeat", b":".to_vec().into());
            message.event_id = Some(id.clone());
            writer
                .append(&CaptureRecord {
                    sequence,
                    exchange_id: 7,
                    kind: CaptureRecordKind::StreamMessage(message),
                })
                .unwrap();
        }
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();
        sizes.push(bytes.len());
        let mut reader = CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        let mut shared = None;
        let mut last = None;
        for _ in 0..count {
            let frame = reader.read_next_indexed().unwrap().unwrap();
            let CaptureRecordKind::StreamMessage(message) = &frame.record.kind else {
                panic!("message")
            };
            let value = message.event_id.as_ref().unwrap();
            assert_eq!(value, &id);
            if let Some(first) = &shared {
                assert!(Arc::ptr_eq(first, value));
            } else {
                shared = Some(value.clone());
            }
            last = Some(frame);
        }
        // The frame keeps its one shared ID alive independently of the reader.
        drop(reader);
        let last = last.unwrap();
        let mut source = Cursor::new(&bytes);
        source.set_position(last.offset);
        let record = super::super::read_indexed_frame_with_codec(
            &mut source,
            last.frame_bytes,
            CaptureLimits::default(),
            &last.codec,
            last.index,
        )
        .unwrap();
        let CaptureRecordKind::StreamMessage(message) = record.kind else {
            panic!("message")
        };
        assert!(Arc::ptr_eq(
            shared.as_ref().unwrap(),
            message.event_id.as_ref().unwrap()
        ));
        assert_eq!(&message.payload[..], b":");
    }
    println!("64 KiB persistent SSE ID, 128/1024 messages: {sizes:?} file bytes");
    assert!(sizes[1] < 1024 * 512 + id.len() * 2);
    assert!(sizes[1] - sizes[0] < (1024 - 128) * 512);

    // Reproduce the previous repeated-field layout with the same compressor.
    let mut legacy = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    let mut message = StreamMessage::new("server", "heartbeat", Vec::new().into());
    message.event_id = Some(id);
    for sequence in 1..=1024 {
        legacy
            .append(&CaptureRecord {
                sequence,
                exchange_id: 7,
                kind: CaptureRecordKind::Unknown {
                    kind: "stream-message".into(),
                    payload: serde_json::to_value(&message).unwrap(),
                },
            })
            .unwrap();
    }
    legacy.seal().unwrap();
    let before = legacy.into_inner().unwrap().len();
    println!(
        "1024 messages, previous repeated-ID layout: {before} bytes; shared-ID layout: {} bytes",
        sizes[1]
    );
    assert!(sizes[1] * 10 < before);
}

#[test]
fn shared_stream_ids_survive_encrypted_compaction_and_empty_resets() {
    use transmog_core::stream::StreamMessage;
    let options = CaptureEncoding {
        password: Some(CapturePassword::new("stream sharing".into())),
    };
    let mut writer =
        CaptureWriter::with_encoding(Cursor::new(Vec::new()), CaptureLimits::default(), &options)
            .unwrap();
    writer.enable_checkpoint_locators();
    writer.checkpoint().unwrap();
    let mut expected = Vec::new();
    for (index, id) in [Some("first"), Some("first"), Some(""), None, Some("first")]
        .into_iter()
        .enumerate()
    {
        let mut message = StreamMessage::new("server", "event", b"payload".to_vec().into());
        message.event_id = id.map(Into::into);
        let record = CaptureRecord {
            sequence: index as u64 + 1,
            exchange_id: 7,
            kind: CaptureRecordKind::StreamMessage(message),
        };
        writer.append(&record).unwrap();
        expected.push(record);
    }
    writer.seal().unwrap();
    let bytes = writer.compact_sealed_bytes().unwrap().unwrap();
    let mut reader = CaptureReader::with_password(
        Cursor::new(&bytes),
        CaptureLimits::default(),
        options.password.as_ref(),
    )
    .unwrap();
    for record in expected {
        assert_eq!(reader.read_next().unwrap().unwrap().record, record);
    }
}
