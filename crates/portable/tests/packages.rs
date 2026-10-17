//! Portable transfer integrity, safe archive boundaries and transactional publication.
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use transmog_capture::{
    CaptureEncoding, CapturePassword,
    artifact::{ArtifactReader, ArtifactWriter},
};
use transmog_portable::{CaseManifest, Package, PackageManifest, PreparedCase, write_case_package};
fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../verification/fixtures/portable")
}
fn prepared(stage: &Path) -> PreparedCase {
    let bytes = std::fs::read(corpus().join("manifests/network-context.json")).unwrap();
    PreparedCase::prepare(
        &bytes,
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
#[test]
fn native_transfers_preserve_bytes_working_copy_and_pinned_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let value = prepared(root.path());
    let flag = AtomicBool::new(false);
    for (i, password) in [None, Some(CapturePassword::new("fixture".into()))]
        .into_iter()
        .enumerate()
    {
        let path = root.path().join(format!("case-{i}"));
        write_case_package(&value, &path, password.clone(), false, &flag).unwrap();
        let package = Package::open(&path, password.as_ref(), &flag).unwrap();
        let PackageManifest::Case(manifest) = package.manifest() else {
            panic!()
        };
        assert_eq!(manifest, value.manifest());
        for object in &manifest.objects {
            let mut actual = vec![];
            let mut reader = package.open_object(&object.sha256).unwrap();
            reader.read_to_end(&mut actual).unwrap();
            assert_eq!(transmog_portable::sha256(&actual), object.sha256);
            reader.seek(SeekFrom::End(-1)).unwrap();
            let mut one = [0];
            reader.read_exact(&mut one).unwrap();
            assert_eq!(one[0], *actual.last().unwrap());
        }
        let work = package
            .save_working_copy(&root.path().join(format!("work-{i}")), root.path(), &flag)
            .unwrap();
        assert_eq!(work.prepared().manifest(), manifest);
        let first = manifest.objects.first().unwrap().clone();
        let mut retained = package.open_object(&first.sha256).unwrap();
        drop(package);
        std::fs::remove_file(&path).unwrap();
        let mut actual = vec![];
        retained.read_to_end(&mut actual).unwrap();
        assert_eq!(transmog_portable::sha256(&actual), first.sha256);
    }
}
#[test]
fn whole_case_password_failure_cancel_and_existing_destination_are_transactional() {
    let root = tempfile::tempdir().unwrap();
    let value = prepared(root.path());
    let flag = AtomicBool::new(false);
    let path = root.path().join("encrypted");
    let password = CapturePassword::new("fixture-secret".into());
    write_case_package(&value, &path, Some(password.clone()), false, &flag).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        Package::open(&path, None, &flag),
        Err(transmog_portable::Error::PasswordRequired)
    ));
    assert!(matches!(
        Package::open(&path, Some(&CapturePassword::new("wrong".into())), &flag),
        Err(transmog_portable::Error::InvalidPassword)
    ));
    let package = Package::open(&path, Some(&password), &flag).unwrap();
    assert_eq!(package.manifest().title(), value.manifest().title);
    assert!(write_case_package(&value, &path, None, false, &flag).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(write_case_package(&value, &path, None, true, &AtomicBool::new(true)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!std::fs::read_dir(root.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".case-transfer-")
    }));
}
#[test]
fn production_reader_accepts_streamed_zip64_and_rejects_hostile_corpus() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("wrapped.tmcap");
    for valid in ["simple", "zip64-streamed"] {
        wrap_zip(
            &path,
            &std::fs::read(corpus().join(format!("packages/{valid}.tmcase"))).unwrap(),
        );
        Package::open(&path, None, &AtomicBool::new(false)).unwrap();
    }
    for invalid in [
        "unlisted-member",
        "traversal",
        "duplicate-member",
        "drive-path",
        "digest-mismatch",
    ] {
        wrap_zip(
            &path,
            &std::fs::read(corpus().join(format!("packages/{invalid}.tmcase"))).unwrap(),
        );
        assert!(
            matches!(
                Package::open(&path, None, &AtomicBool::new(false)),
                Err(transmog_portable::Error::Invalid(_))
            ),
            "{invalid}"
        );
    }
}
// Raw ZIP fixtures exercise the inner archive parser through a fully valid outer container.
fn wrap_zip(path: &Path, bytes: &[u8]) {
    let mut writer = ArtifactWriter::new(
        File::create(path).unwrap(),
        "application/vnd.transmog.case+zip",
        &CaptureEncoding::default(),
        true,
    )
    .unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap();
}
fn inner_zip(path: &Path, password: Option<&CapturePassword>) -> Vec<u8> {
    let mut reader = ArtifactReader::new(File::open(path).unwrap(), password).unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    bytes
}
#[test]
fn bare_case_archives_are_unsupported_even_with_native_extensions() {
    let root = tempfile::tempdir().unwrap();
    let bytes = std::fs::read(corpus().join("packages/simple.tmcase")).unwrap();
    for name in ["case.tmcase.zip", "renamed.tmcap"] {
        let path = root.path().join(name);
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            Package::open(&path, None, &AtomicBool::new(false)),
            Err(transmog_portable::Error::Unsupported(_))
        ));
    }
}
#[test]
fn corrupt_and_incomplete_transfers_do_not_publish_working_directories() {
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(root.path());
    let path = root.path().join("case");
    let flag = AtomicBool::new(false);
    write_case_package(&prepared, &path, None, false, &flag).unwrap();
    let bytes = inner_zip(&path, None);
    for (name, changed) in [
        ("truncated", bytes[..bytes.len() - 10].to_vec()),
        ("trailing", [bytes.as_slice(), b"x"].concat()),
    ] {
        let path = root.path().join(name);
        wrap_zip(&path, &changed);
        assert!(Package::open(&path, None, &flag).is_err());
    }
    let mut changed = bytes.clone();
    changed[100] ^= 1;
    let path = root.path().join("corrupt");
    wrap_zip(&path, &changed);
    assert!(Package::open(&path, None, &flag).is_err());
    let manifest = CaseManifest::parse(&prepared.manifest().encode().unwrap()).unwrap();
    assert_eq!(manifest.title, prepared.manifest().title);
}

#[test]
fn standards_reader_opens_native_rule_payload_with_current_timestamps() {
    let root = tempfile::tempdir().unwrap();
    let flag = AtomicBool::new(false);
    let bytes = std::fs::read(corpus().join("manifests/rules-only.json")).unwrap();
    let rules = transmog_portable::PreparedCollection::prepare(
        &bytes,
        root.path(),
        |sha| {
            Ok(Box::new(File::open(
                corpus().join("objects/sha256").join(sha),
            )?))
        },
        &flag,
    )
    .unwrap();
    let wrapped = root.path().join("rules.tmrules.tmcap");
    transmog_portable::write_rules_package(&rules, &wrapped, None, false, &flag).unwrap();
    let inner = ArtifactReader::new(File::open(&wrapped).unwrap(), None).unwrap();
    let mut standard = zip::ZipArchive::new(inner).unwrap();
    assert_eq!(standard.len(), rules.manifest().objects.len() + 1);
    let now = time::OffsetDateTime::now_utc();
    for index in 0..standard.len() {
        let member = standard.by_index(index).unwrap();
        let modified = member.last_modified().unwrap();
        let stamp = time::Date::from_calendar_date(
            modified.year().into(),
            modified.month().try_into().unwrap(),
            modified.day(),
        )
        .unwrap()
        .with_hms(modified.hour(), modified.minute(), modified.second())
        .unwrap()
        .assume_utc();
        assert!(
            (now - stamp).whole_seconds().abs() < 60,
            "ZIP member has stale timestamp"
        );
    }
    let mut actual = vec![];
    standard
        .by_name("manifest.json")
        .unwrap()
        .read_to_end(&mut actual)
        .unwrap();
    assert_eq!(actual, rules.manifest().encode().unwrap());
    assert!(matches!(
        Package::open(&wrapped, None, &flag).unwrap().manifest(),
        PackageManifest::Rules(_)
    ));
    let bytes = inner_zip(&wrapped, None);
    for name in ["rules.tmrules.zip", "renamed-rules.tmcap"] {
        let path = root.path().join(name);
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            Package::open(&path, None, &flag),
            Err(transmog_portable::Error::Unsupported(_))
        ));
    }
}

#[test]
fn forwarding_preserves_exact_inner_zip_and_uses_a_separate_new_output_password() {
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(root.path());
    let flag = AtomicBool::new(false);
    let source = root.path().join("original.tmcase.tmcap");
    write_case_package(&prepared, &source, None, false, &flag).unwrap();
    let original = inner_zip(&source, None);
    let package = Package::open(&source, None, &flag).unwrap();
    let path = root.path().join("protected");
    let first = CapturePassword::new("first-output".into());
    package
        .write_transfer(&path, Some(&first), false, &flag)
        .unwrap();
    let protected = Package::open(&path, Some(&first), &flag).unwrap();
    assert!(protected.encrypted());
    let reader =
        transmog_capture::artifact::ArtifactReader::new(File::open(&path).unwrap(), Some(&first))
            .unwrap();
    assert_eq!(
        reader.descriptor().sha256,
        transmog_portable::sha256(&original)
    );
    drop(reader);
    let before = std::fs::read(&path).unwrap();
    let second = CapturePassword::new("separate-new-output".into());
    // Replacing the pinned input keeps old readers on the original snapshot.
    let forwarded = path.clone();
    protected
        .write_transfer(&forwarded, Some(&second), true, &flag)
        .unwrap();
    assert_ne!(std::fs::read(&forwarded).unwrap(), before);
    assert!(Package::open(&forwarded, Some(&first), &flag).is_err());
    let second_package = Package::open(&forwarded, Some(&second), &flag).unwrap();
    assert_eq!(
        second_package.manifest().encode().unwrap(),
        protected.manifest().encode().unwrap()
    );
    let copied = root.path().join("unprotected.tmcase.tmcap");
    second_package
        .write_transfer(&copied, None, false, &flag)
        .unwrap();
    assert!(!Package::open(&copied, None, &flag).unwrap().encrypted());
    assert_eq!(inner_zip(&copied, None), original);
    assert_eq!(inner_zip(&source, None), original);
    let first_object = &prepared.manifest().objects[0];
    let mut bytes = vec![];
    protected
        .open_object(&first_object.sha256)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(transmog_portable::sha256(&bytes), first_object.sha256);
}

#[cfg(windows)]
#[test]
fn blocked_replacement_preserves_existing_output_and_removes_owned_staging() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(root.path());
    let flag = AtomicBool::new(false);
    let target = root.path().join("existing");
    std::fs::write(&target, b"existing evidence").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&target)
        .unwrap();
    assert!(write_case_package(&prepared, &target, None, true, &flag).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"existing evidence");
    assert!(!std::fs::read_dir(root.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".case-transfer-")
    }));
    drop(held);
    write_case_package(&prepared, &target, None, true, &flag).unwrap();
    Package::open(&target, None, &flag).unwrap();
}
