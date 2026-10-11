use super::*;
use serde_json::json;
use std::io::Cursor;

fn malformed_blocks(options: &CaptureEncoding, blocks: &[(u8, serde_json::Value)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let (codec, _) = FrameCodec::create(&mut bytes, options).unwrap();
    bytes.extend_from_slice(&[0; LOCATORS]);
    for (ordinal, (kind, value)) in blocks.iter().enumerate() {
        let json = serde_json::to_vec(value).unwrap();
        let mut packed = vec![0];
        packed.extend_from_slice(&u32::try_from(json.len()).unwrap().to_le_bytes());
        packed.extend_from_slice(&json);
        let mut plain = vec![0; BLOCK_PLAIN];
        plain[0] = *kind;
        plain[1..5].copy_from_slice(&u32::try_from(packed.len()).unwrap().to_le_bytes());
        plain[5..5 + packed.len()].copy_from_slice(&packed);
        bytes.extend(codec.encode_block(ordinal as u64, &plain).unwrap());
    }
    bytes
}

#[test]
fn hostile_branch_level_returns_an_error_without_panicking() {
    for password in [None, Some(CapturePassword::new("malformed branch".into()))] {
        let children = json!([{"block":0,"blocks":1,"bytes":7,"first":0,"count":1,"level":255}]);
        let footer = json!({"roots":[{"block":1,"blocks":1,"bytes":serde_json::to_vec(&children).unwrap().len()+5,
            "first":0,"count":1,"level":1}],"count":1,"sealed":false,"block":2});
        let bytes = malformed_blocks(
            &CaptureEncoding {
                password: password.clone(),
            },
            &[(LEAF, json!([])), (BRANCH, children), (FOOTER, footer)],
        );
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
        let bytes = malformed_blocks(
            &CaptureEncoding {
                password: password.clone(),
            },
            &[(
                FOOTER,
                json!({"roots":[],"count":0,"sealed":true,"block":0}),
            )],
        );
        assert!(
            CaptureReader::with_password(
                Cursor::new(bytes),
                CaptureLimits::default(),
                password.as_ref()
            )
            .is_err()
        );
    }
}

#[test]
fn unsealed_empty_checkpoint_remains_valid() {
    let bytes = malformed_blocks(
        &CaptureEncoding::default(),
        &[(
            FOOTER,
            json!({"roots":[],"count":0,"sealed":false,"block":0}),
        )],
    );
    let mut reader = CaptureReader::new(Cursor::new(bytes), CaptureLimits::default()).unwrap();
    assert!(!reader.sealed());
    assert!(reader.read_next().unwrap().is_none());
}

fn reference_fixture(change: impl FnOnce(&mut Vec<Entry>, u64)) -> Vec<u8> {
    let mut bytes = Vec::new();
    let (codec, header) = FrameCodec::create(&mut bytes, &CaptureEncoding::default()).unwrap();
    bytes.extend_from_slice(&[0; LOCATORS]);
    let start = header + LOCATORS as u64;
    let mut entries = vec![Entry {
        offset: start + 5,
        frame_bytes: 100,
        index: 0,
        stored: store(&CaptureRecord {
            sequence: 1,
            exchange_id: 1,
            kind: CaptureRecordKind::Completed,
        })
        .unwrap(),
    }];
    change(&mut entries, start);
    let count = entries.len();
    let mut packed = vec![0];
    let json = serde_json::to_vec(&entries).unwrap();
    packed.extend_from_slice(&u32::try_from(json.len()).unwrap().to_le_bytes());
    packed.extend(json);
    let footer = serde_json::to_vec(&json!({"roots":[{"block":1,"blocks":1,"bytes":packed.len(),
        "first":0,"count":count,"level":0}],"count":count,"sealed":false,"block":2}))
    .unwrap();
    let mut commit = vec![0];
    commit.extend_from_slice(&u32::try_from(footer.len()).unwrap().to_le_bytes());
    commit.extend(footer);
    for (ordinal, (kind, payload)) in [(DATA, vec![0; 1000]), (LEAF, packed), (FOOTER, commit)]
        .into_iter()
        .enumerate()
    {
        let mut plain = vec![0; BLOCK_PLAIN];
        plain[0] = kind;
        plain[1..5].copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        plain[5..5 + payload.len()].copy_from_slice(&payload);
        bytes.extend(codec.encode_block(ordinal as u64, &plain).unwrap());
    }
    bytes
}

#[test]
fn index_rejects_impossible_data_references_without_reading_bodies() {
    for scenario in 0..6 {
        let bytes = reference_fixture(|entries, start| match scenario {
            0 => entries[0].offset = start,
            1 => entries[0].offset = start + BLOCK_PLAIN as u64,
            2 => entries[0].offset = start + BLOCK_BYTES as u64 + 5,
            3 => entries[0].frame_bytes = 0,
            4 => entries[0].frame_bytes = PAYLOAD as u64 + 1,
            _ => {
                let mut duplicate = entries[0].clone();
                duplicate.index = 1;
                entries.push(duplicate);
            }
        });
        let mut reader = CaptureReader::new(Cursor::new(bytes), CaptureLimits::default()).unwrap();
        let result = (|| {
            while reader.read_next_indexed()?.is_some() {}
            Ok::<_, CaptureError>(())
        })();
        assert!(
            result.is_err(),
            "accepted impossible reference scenario {scenario}"
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
