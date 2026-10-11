//! Semantic fuzz seeds must keep reaching their intended current-format reader paths.
#[path = "../../../fuzz/support/capture_crypto.rs"]
mod crypto_harness;
#[path = "../../../fuzz/support/capture_reader.rs"]
mod reader_harness;

use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::Cursor, path::PathBuf};
use transmog_capture::{CaptureError, CaptureReader, CaptureRecordKind};

fn fuzz_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz")
}
fn seeds(target: &str) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(fuzz_root().join("seeds").join(target))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
fn error_name(error: &CaptureError) -> String {
    match error {
        CaptureError::MalformedRecord(reason) => format!("MalformedRecord: {reason}"),
        CaptureError::Json(_) => "Json".into(),
        CaptureError::Io(_) => "Io".into(),
        _ => format!("{error:?}"),
    }
}
fn probe(input: &[u8], indexed: bool) -> Value {
    let bytes = reader_harness::fixture(input).expect("seed must construct a fixture");
    let limits = reader_harness::limits(input[0]);
    let mut reader = match CaptureReader::new(Cursor::new(&bytes), limits) {
        Ok(reader) => reader,
        Err(error) => return json!({"stage":"open", "error":error_name(&error)}),
    };
    let mut kinds = Vec::new();
    loop {
        let next = if indexed {
            reader.read_next_indexed()
        } else {
            reader.read_next()
        };
        match next {
            Ok(Some(frame)) => {
                let kind = match frame.record.kind {
                    CaptureRecordKind::Unknown { kind, .. } => kind,
                    CaptureRecordKind::Performance(_) => "performance".into(),
                    CaptureRecordKind::Completed => "completed".into(),
                    CaptureRecordKind::Seal { .. } => "seal".into(),
                    CaptureRecordKind::BodySegment {
                        bytes, byte_count, ..
                    } => {
                        if !indexed {
                            assert_eq!(bytes.as_ref().unwrap().len(), byte_count);
                        }
                        format!("body-segment:{byte_count}")
                    }
                    other => panic!("classify new seed payload: {other:?}"),
                };
                kinds.push(kind);
            }
            Ok(None) => return json!({"records":kinds,"sealed":reader.sealed()}),
            Err(error) => {
                return json!({"stage":if indexed {"index"} else {"full"},
                "records":kinds, "error":error_name(&error)});
            }
        }
    }
}

#[test]
fn reader_seeds_reach_their_documented_stage() {
    let mut actual = BTreeMap::new();
    for (name, input) in seeds("capture_reader") {
        actual.insert(
            name,
            json!({"indexed":probe(&input,true), "full":probe(&input,false)}),
        );
        reader_harness::exercise(&input);
    }
    let expected_path = fuzz_root().join("capture-seed-expectations.json");
    let actual = serde_json::to_value(actual).unwrap();
    // Never silently bless changed results: both additions and semantic drift need review.
    let expected: Value = serde_json::from_slice(&fs::read(expected_path).unwrap_or_else(|_| {
        panic!(
            "Review seed expectations before installing them:\n{}",
            serde_json::to_string_pretty(&actual).unwrap()
        );
    }))
    .unwrap();
    assert_eq!(
        actual, expected,
        "TMCap seed coverage changed; review the seed and expected stage"
    );
}

#[test]
fn crypto_seeds_exercise_all_body_and_context_oracles() {
    let inputs = seeds("capture_crypto");
    let mut modes = [false; 8];
    for input in inputs.values() {
        assert!(
            input.len() >= 4,
            "crypto seed cannot return before its oracle"
        );
        modes[usize::from(input[0] & 7)] = true;
        crypto_harness::exercise(input);
    }
    assert!(modes.into_iter().all(|seen| seen));
}
