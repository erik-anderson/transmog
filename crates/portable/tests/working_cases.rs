//! Production case validation, bounded object snapshots and transactional working copies.
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use transmog_portable::{CaseManifest, PreparedCase, WorkingCase};
fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../verification/fixtures/portable")
}
fn manifest(name: &str) -> CaseManifest {
    CaseManifest::parse(
        &std::fs::read(corpus().join("manifests").join(format!("{name}.json"))).unwrap(),
    )
    .unwrap()
}
fn prepare(value: &CaseManifest, stage: &Path) -> PreparedCase {
    PreparedCase::prepare(
        &value.encode().unwrap(),
        stage,
        |sha| {
            Ok(Box::new(File::open(
                corpus().join("objects/sha256").join(sha),
            )?))
        },
        &AtomicBool::new(false),
    )
    .unwrap()
}
fn next(case: &WorkingCase, stage: &Path) -> PreparedCase {
    let mut value = case.prepared().manifest().clone();
    value.revision = (value.revision.parse::<u64>().unwrap() + 1).to_string();
    value.notes[0].text.push_str(" Edited.");
    PreparedCase::prepare(
        &value.encode().unwrap(),
        stage,
        |sha| Ok(Box::new(case.prepared().open_object(sha)?)),
        &AtomicBool::new(false),
    )
    .unwrap()
}
#[test]
fn saved_comparisons_validate_membership_corrections_and_representation_without_trusting_findings()
{
    use transmog_portable::{
        CaseComparison, ComparisonPair, ComparisonSettings, EvidenceReference,
    };
    let mut value = manifest("source-collision");
    let refs = value
        .sources
        .iter()
        .map(|s| EvidenceReference {
            artifact_sha256: s.artifact_sha256.clone(),
            source_key: s.source_key.clone(),
            exchange_id: Some("original".into()),
            occurrence: "0".into(),
            boundary: "client-response".into(),
        })
        .collect::<Vec<_>>();
    value.comparisons = vec![CaseComparison {
        id: "saved-comparison".into(),
        label: "Working / failing".into(),
        kind: "captures".into(),
        engine: "future-engine/2".into(),
        left: vec![refs[0].clone()],
        right: vec![refs[1].clone()],
        request_boundary: "client-request".into(),
        settings: ComparisonSettings {
            excluded_headers: vec![],
            normalize_newlines: false,
            excluded_query_names: vec!["nonce".into()],
            request_headers: vec![],
            request_body_digest: false,
        },
        corrections: vec![ComparisonPair {
            left: Some(refs[0].clone()),
            right: Some(refs[1].clone()),
        }],
        recorded_summary: "Historical finding <script>inert</script>".into(),
    }];
    // Unknown engine versions remain inspectable; execution adapters must reject recomputation.
    assert_eq!(
        CaseManifest::parse(&value.encode().unwrap()).unwrap(),
        value
    );
    let mut invalid = value.clone();
    invalid.comparisons[0].left.push(refs[0].clone());
    assert!(invalid.encode().is_err());
    invalid = value.clone();
    invalid.comparisons[0].left[0].boundary = "upstream-response".into();
    assert!(invalid.encode().is_err());
    invalid = value.clone();
    invalid.comparisons[0].corrections[0].right = Some(refs[0].clone());
    assert!(invalid.encode().is_err());
    invalid = value.clone();
    let repeated = invalid.comparisons[0].corrections[0].clone();
    invalid.comparisons[0].corrections.push(repeated);
    assert!(invalid.encode().is_err());
    invalid = value.clone();
    invalid.comparisons[0].settings.normalize_newlines = true;
    assert!(invalid.encode().is_err());
    invalid = value.clone();
    invalid.comparisons[0].left[0].artifact_sha256 = "a".repeat(64);
    assert!(invalid.encode().is_err());
    invalid = value.clone();
    invalid.comparisons[0].kind = "exchange".into();
    assert!(invalid.encode().is_err());
}
#[test]
fn production_case_reader_verifies_the_current_corpus_and_keeps_availability_distinct() {
    let stage = tempfile::tempdir().unwrap();
    for name in [
        "simple-css",
        "source-collision",
        "coded-duplicate-headers",
        "network-context",
        "sensitive-notes",
        "encrypted-capture",
        "derived-evidence",
        "missing-asset",
        "linked-file-changed",
        "future-optional",
    ] {
        let value = manifest(name);
        let inventory = value.inventory().unwrap();
        assert!(inventory.sources > 0);
        let prepared = prepare(&value, stage.path());
        assert_eq!(prepared.manifest(), &value);
        for object in &value.objects {
            let mut bytes = Vec::new();
            prepared
                .open_object(&object.sha256)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            assert_eq!(transmog_portable::sha256(&bytes), object.sha256);
        }
    }
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
    let missing = manifest("missing-asset");
    assert!(missing.inventory().unwrap().missing_dependencies > 0);
    assert!(
        missing
            .rules_for_import(&["experiment-css".into()], false)
            .is_err()
    );
    let future = manifest("future-optional");
    assert_eq!(
        future.inventory().unwrap().experiments[1].state,
        "unavailable"
    );
    assert!(
        future
            .rules_for_import(&["future-optional".into()], false)
            .is_err()
    );
    assert!(
        future
            .rules_for_import(&["experiment-css".into()], false)
            .is_ok()
    );
    let run = manifest("linked-file-changed");
    assert_ne!(
        run.runs[0].responses[0].body.sha256,
        run.assets[0].body.sha256
    );
    for name in ["future-version", "future-required"] {
        assert!(matches!(
            CaseManifest::parse(
                &std::fs::read(corpus().join("manifests").join(format!("{name}.json"))).unwrap()
            ),
            Err(transmog_portable::Error::Unsupported(_))
        ));
    }
}
#[test]
fn working_copy_saves_history_rejects_stale_writers_and_pins_owned_objects() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let path = root.path().join("café case");
    let mut first = WorkingCase::create(
        prepare(&manifest("network-context"), stage.path()),
        &path,
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut second = WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).unwrap();
    first
        .save(next(&first, stage.path()), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(first.history(), ["1"]);
    let original = first
        .open_revision("1", stage.path(), &AtomicBool::new(false))
        .unwrap();
    assert!(!original.manifest().notes[0].text.ends_with(" Edited."));
    assert!(
        second
            .save(next(&second, stage.path()), &AtomicBool::new(false))
            .is_err()
    );
    let reopened = WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).unwrap();
    assert_eq!(reopened.prepared().manifest().revision, "2");
    let hash = std::fs::read(path.join("manifest.json")).unwrap();
    assert!(
        first
            .save(next(&first, stage.path()), &AtomicBool::new(true))
            .is_err()
    );
    assert_eq!(std::fs::read(path.join("manifest.json")).unwrap(), hash);
    first.prune_history(&["1".into()]).unwrap();
    assert_eq!(first.history(), [] as [String; 0]);
    assert!(
        WorkingCase::create(
            prepare(&manifest("simple-css"), stage.path()),
            &path,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    std::fs::remove_dir_all(&path).unwrap();
    assert!(
        first
            .prepared()
            .open_object(&first.prepared().manifest().objects[0].sha256)
            .is_ok()
    );
}
#[test]
fn malformed_reference_and_corruption_never_publish_a_partial_case() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let mut value = manifest("simple-css");
    value.notes[0].source.as_mut().unwrap().artifact_sha256 = "f".repeat(64);
    assert!(value.encode().is_err());
    let value = manifest("simple-css");
    assert!(
        PreparedCase::prepare(
            &value.encode().unwrap(),
            stage.path(),
            |_| Ok(Box::new(std::io::Cursor::new(b"bad"))),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
    let path = root.path().join("case");
    WorkingCase::create(
        prepare(&value, stage.path()),
        &path,
        &AtomicBool::new(false),
    )
    .unwrap();
    std::fs::write(
        path.join("objects/sha256").join(&value.objects[0].sha256),
        b"tampered",
    )
    .unwrap();
    assert!(WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).is_err());
}
#[test]
fn owned_staging_remnants_are_reported_without_becoming_a_manifest() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let path = root.path().join("case");
    WorkingCase::create(
        prepare(&manifest("simple-css"), stage.path()),
        &path,
        &AtomicBool::new(false),
    )
    .unwrap();
    std::fs::write(path.join(".case-manifest-interrupted"), b"not JSON").unwrap();
    let reopened = WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).unwrap();
    assert_eq!(reopened.recovery_artifacts(), 1);
    assert_eq!(reopened.prepared().manifest().revision, "1");
    std::fs::create_dir(path.join("history/1")).unwrap();
    let recovered = WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).unwrap();
    assert_eq!(recovered.history(), [] as [String; 0]);
    assert_eq!(recovered.recovery_artifacts(), 2);
    std::fs::write(
        path.join("history/1/manifest.json"),
        b"interrupted history write",
    )
    .unwrap();
    assert!(WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).is_ok());
    std::fs::write(path.join("unexpected.txt"), b"foreign").unwrap();
    assert!(WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).is_err());
}
#[cfg(windows)]
#[test]
fn blocked_manifest_replacement_preserves_current_case_and_can_retry() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let path = root.path().join("case");
    let mut case = WorkingCase::create(
        prepare(&manifest("simple-css"), stage.path()),
        &path,
        &AtomicBool::new(false),
    )
    .unwrap();
    let bytes = std::fs::read(path.join("manifest.json")).unwrap();
    let held = File::options()
        .read(true)
        .share_mode(3)
        .open(path.join("manifest.json"))
        .unwrap();
    assert!(
        case.save(next(&case, stage.path()), &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(std::fs::read(path.join("manifest.json")).unwrap(), bytes);
    drop(held);
    assert_eq!(
        WorkingCase::open(&path, stage.path(), &AtomicBool::new(false))
            .unwrap()
            .prepared()
            .manifest()
            .revision,
        "1"
    );
    case.save(next(&case, stage.path()), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(case.prepared().manifest().revision, "2");
}
#[test]
fn committed_history_is_bounded_and_explicit_pruning_releases_a_revision_slot() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let path = root.path().join("case");
    let mut case = WorkingCase::create(
        prepare(&manifest("simple-css"), stage.path()),
        &path,
        &AtomicBool::new(false),
    )
    .unwrap();
    for _ in 0..31 {
        case.save(next(&case, stage.path()), &AtomicBool::new(false))
            .unwrap();
    }
    assert_eq!(case.history().len(), 31);
    assert_eq!(case.prepared().manifest().revision, "32");
    assert!(
        case.save(next(&case, stage.path()), &AtomicBool::new(false))
            .is_err()
    );
    case.prune_history(&["1".into()]).unwrap();
    case.save(next(&case, stage.path()), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(case.prepared().manifest().revision, "33");
    std::fs::create_dir(path.join("history/1")).unwrap();
    let reopened = WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).unwrap();
    assert_eq!(reopened.history().len(), 31);
    assert_eq!(reopened.recovery_artifacts(), 1);
}

#[test]
fn large_case_objects_stream_in_bounded_chunks() {
    use sha2::{Digest as _, Sha256};
    struct Zeros {
        remaining: usize,
    }
    impl Read for Zeros {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            assert!(buffer.len() <= 128 * 1024);
            let count = buffer.len().min(self.remaining);
            buffer[..count].fill(0);
            self.remaining -= count;
            Ok(count)
        }
    }
    let length = 17 * 1024 * 1024;
    let mut hash = Sha256::new();
    let zeros = vec![0_u8; 128 * 1024];
    for _ in 0..136 {
        hash.update(&zeros);
    }
    let digest = format!("{:x}", hash.finalize());
    let original = manifest("simple-css");
    let prior = &original.sources[0].artifact_sha256;
    let bytes = String::from_utf8(original.encode().unwrap())
        .unwrap()
        .replace(prior, &digest);
    let mut value: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    value["objects"][0]["bytes"] = length.to_string().into();
    let value = CaseManifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    let stage = tempfile::tempdir().unwrap();
    let case = PreparedCase::prepare(
        &value.encode().unwrap(),
        stage.path(),
        |sha| {
            if sha == digest {
                Ok(Box::new(Zeros { remaining: length }) as Box<dyn Read + Send>)
            } else {
                Ok(Box::new(File::open(
                    corpus().join("objects/sha256").join(sha),
                )?))
            }
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        case.open_object(&digest).unwrap().metadata().unwrap().len(),
        length as u64
    );
}

#[cfg(unix)]
#[test]
fn object_and_directory_links_are_rejected() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let path = root.path().join("case");
    let value = manifest("simple-css");
    WorkingCase::create(
        prepare(&value, stage.path()),
        &path,
        &AtomicBool::new(false),
    )
    .unwrap();
    let object = path.join("objects/sha256").join(&value.objects[0].sha256);
    std::fs::remove_file(&object).unwrap();
    symlink(
        corpus()
            .join("objects/sha256")
            .join(&value.objects[0].sha256),
        &object,
    )
    .unwrap();
    assert!(WorkingCase::open(&path, stage.path(), &AtomicBool::new(false)).is_err());
}
