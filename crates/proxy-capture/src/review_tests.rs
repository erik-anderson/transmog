use super::*;
use std::io::Cursor;

#[test]
fn hostile_branch_level_returns_an_error_without_panicking() {
    for password in [None, Some(CapturePassword::new("malformed branch".into()))] {
        let options = CaptureEncoding {
            password: password.clone(),
        };
        let mut writer =
            CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options).unwrap();
        writer.small = None;
        let mut child = writer.object(LEAF, b"[]", 0, 1, 0).unwrap();
        child.level = 255;
        let root = writer
            .object(BRANCH, &serde_json::to_vec(&vec![child]).unwrap(), 0, 1, 1)
            .unwrap();
        writer.levels = vec![vec![root]];
        writer.records = 1;
        let bytes = writer.into_inner().unwrap();
        let mut reader = CaptureReader::with_password(
            Cursor::new(bytes),
            CaptureLimits::default(),
            password.as_ref(),
        )
        .unwrap();
        assert!(reader.read_next_indexed().is_err());
    }
}
#[test]
fn sealed_footer_cannot_omit_the_seal_record() {
    for password in [None, Some(CapturePassword::new("missing seal".into()))] {
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding {
                password: password.clone(),
            },
        )
        .unwrap();
        writer.small = None;
        writer.commit(true).unwrap();
        writer.sealed = true;
        assert!(
            CaptureReader::with_password(
                Cursor::new(writer.into_inner().unwrap()),
                CaptureLimits::default(),
                password.as_ref()
            )
            .is_err()
        );
    }
}
#[test]
fn unsealed_empty_checkpoint_remains_valid() {
    let writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    let mut reader = CaptureReader::new(
        Cursor::new(writer.into_inner().unwrap()),
        CaptureLimits::default(),
    )
    .unwrap();
    assert!(!reader.sealed());
    assert!(reader.read_next().unwrap().is_none());
}
fn reference_fixture(change: impl FnOnce(&mut Entry, u64)) -> Vec<u8> {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    writer.small = None;
    writer
        .append(&super::tests::body(1, vec![42; 1000]))
        .unwrap();
    change(&mut writer.entries[0], writer.start);
    writer.into_inner().unwrap()
}
#[test]
fn index_rejects_impossible_data_references_without_reading_bodies() {
    for scenario in 0..9 {
        let bytes = reference_fixture(|entry, start| match scenario {
            0 => entry.body.as_mut().unwrap().offset = start,
            1 => entry.body.as_mut().unwrap().offset = start + BLOCK_PLAIN as u64,
            2 => entry.body.as_mut().unwrap().offset = start + BLOCK_BYTES as u64 + 5,
            3 => entry.stored.body_index.as_mut().unwrap().frame_bytes = 0,
            4 => entry.stored.body_index.as_mut().unwrap().frame_bytes = PAYLOAD as u64 + 1,
            5 => entry.body = None,
            6 => {
                entry.body.as_mut().unwrap().dictionary = Some(packed::BaseRef {
                    offset: start + 5,
                    descriptor: entry.stored.body_index.unwrap(),
                });
            }
            7 => entry.stored.body_index = None,
            _ => entry.body.as_mut().unwrap().offset = u64::MAX,
        });
        let mut reader = CaptureReader::new(Cursor::new(bytes), CaptureLimits::default()).unwrap();
        assert!(
            reader.read_next_indexed().is_err(),
            "accepted impossible reference {scenario}"
        );
    }
}
#[test]
fn caller_supplied_seals_cannot_create_an_unreadable_capture() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    assert!(
        writer
            .append(&CaptureRecord {
                sequence: 0,
                exchange_id: 0,
                kind: CaptureRecordKind::Seal { record_count: 0 },
            })
            .is_err()
    );
}

#[test]
fn leaf_count_is_bounded_before_page_deserialization() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    writer.small = None;
    let count = metadata::LEAF_ENTRIES as u64 + 1;
    let leaf = writer.object(LEAF, b"[]", 0, count, 0).unwrap();
    writer.levels = vec![vec![leaf]];
    writer.records = count;
    assert!(
        CaptureReader::new(
            Cursor::new(writer.into_inner().unwrap()),
            CaptureLimits::default()
        )
        .is_err()
    );
}

#[test]
fn oversized_later_payload_does_not_prevent_random_access_to_bounded_record() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    writer.small = None;
    let oversized = format!("{}null{}", "[".repeat(256), "]".repeat(256));
    let page = format!(
        r#"[{{"index":0,"stored":{{"revision":1,"sequence":0,"exchange_id":1,"kind":"completed","payload":null}}}},{{"index":1,"stored":{oversized}}}]"#
    );
    let leaf = writer.object(LEAF, page.as_bytes(), 0, 2, 0).unwrap();
    writer.levels = vec![vec![leaf]];
    writer.records = 2;
    let bytes = writer.into_inner().unwrap();
    let limits = CaptureLimits {
        max_record_bytes: 128,
        ..CaptureLimits::default()
    };
    let mut reader = CaptureReader::new(Cursor::new(&bytes), limits).unwrap();
    let first = reader.read_next_indexed().unwrap().unwrap();
    let mut cursor = Cursor::new(&bytes);
    cursor.set_position(first.offset);
    let random =
        read_indexed_frame_with_codec(cursor, first.frame_bytes, limits, &first.codec, first.index)
            .unwrap();
    assert_eq!(first.record, random);
    assert!(matches!(
        reader.read_next_indexed(),
        Err(CaptureError::RecordTooLarge { .. })
    ));
    let mut cursor = Cursor::new(&bytes);
    cursor.set_position(first.offset);
    assert!(matches!(
        read_indexed_frame_with_codec(
            cursor,
            first.frame_bytes,
            CaptureLimits {
                max_file_bytes: bytes.len() as u64 - 1,
                ..limits
            },
            &first.codec,
            first.index
        ),
        Err(CaptureError::QuotaExceeded)
    ));
}

#[test]
fn writer_rejects_metadata_that_exceeds_reader_allocation_budget() {
    let limits = CaptureLimits {
        max_record_bytes: 128 * 1024,
        ..CaptureLimits::default()
    };
    let mut writer = CaptureWriter::new(Vec::new(), limits).unwrap();
    let payload = serde_json::from_str(&format!("[{}null]", r#"{"a":0},"#.repeat(2048))).unwrap();
    let record = CaptureRecord {
        sequence: 0,
        exchange_id: 1,
        kind: CaptureRecordKind::Unknown {
            kind: "extension".into(),
            payload,
        },
    };
    assert!(matches!(
        writer.append(&record),
        Err(CaptureError::QuotaExceeded)
    ));
}
