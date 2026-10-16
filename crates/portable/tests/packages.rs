//! Portable transfer integrity, safe archive boundaries and transactional publication.
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use transmog_capture::CapturePassword;
use transmog_portable::{
    CaseManifest, Package, PackageFormat, PackageManifest, PreparedCase, write_case_package,
};
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
fn native_and_bare_transfers_preserve_bytes_working_copy_and_pinned_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let value = prepared(root.path());
    let flag = AtomicBool::new(false);
    for (i, format) in [PackageFormat::Native, PackageFormat::BareZip]
        .into_iter()
        .enumerate()
    {
        let path = root.path().join(format!("case-{i}"));
        write_case_package(&value, &path, format, None, false, &flag).unwrap();
        let package = Package::open(&path, None, &flag).unwrap();
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
    write_case_package(
        &value,
        &path,
        PackageFormat::Native,
        Some(password.clone()),
        false,
        &flag,
    )
    .unwrap();
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
    assert!(write_case_package(&value, &path, PackageFormat::BareZip, None, false, &flag).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(
        write_case_package(
            &value,
            &path,
            PackageFormat::BareZip,
            Some(password),
            true,
            &flag
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(
        write_case_package(
            &value,
            &path,
            PackageFormat::Native,
            None,
            true,
            &AtomicBool::new(true)
        )
        .is_err()
    );
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
    for valid in ["simple", "zip64-streamed"] {
        Package::open(
            &corpus().join(format!("packages/{valid}.tmcase")),
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
    }
    for invalid in [
        "unlisted-member",
        "traversal",
        "duplicate-member",
        "drive-path",
        "digest-mismatch",
    ] {
        assert!(
            Package::open(
                &corpus().join(format!("packages/{invalid}.tmcase")),
                None,
                &AtomicBool::new(false)
            )
            .is_err(),
            "{invalid}"
        );
    }
}
#[test]
fn corrupt_and_incomplete_transfers_do_not_publish_working_directories() {
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(root.path());
    let path = root.path().join("case");
    let flag = AtomicBool::new(false);
    write_case_package(&prepared, &path, PackageFormat::BareZip, None, false, &flag).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for (name, changed) in [
        ("truncated", bytes[..bytes.len() - 10].to_vec()),
        ("trailing", [bytes.as_slice(), b"x"].concat()),
    ] {
        let path = root.path().join(name);
        std::fs::write(&path, changed).unwrap();
        assert!(Package::open(&path, None, &flag).is_err());
    }
    let mut changed = bytes.clone();
    changed[100] ^= 1;
    let path = root.path().join("corrupt");
    std::fs::write(&path, changed).unwrap();
    assert!(Package::open(&path, None, &flag).is_err());
    let manifest = CaseManifest::parse(&prepared.manifest().encode().unwrap()).unwrap();
    assert_eq!(manifest.title, prepared.manifest().title);
}

#[test]
fn standards_reader_opens_both_rule_archives_and_the_exact_unwrapped_zip() {
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
    let bare = root.path().join("rules.tmrules.zip");
    transmog_portable::write_rules_package(
        &rules,
        &bare,
        PackageFormat::BareZip,
        None,
        false,
        &flag,
    )
    .unwrap();
    let mut standard = zip::ZipArchive::new(File::open(&bare).unwrap()).unwrap();
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
    let wrapped = root.path().join("rules.tmrules.tmcap");
    transmog_portable::write_rules_package(
        &rules,
        &wrapped,
        PackageFormat::Native,
        None,
        false,
        &flag,
    )
    .unwrap();
    let inner =
        transmog_capture::artifact::ArtifactReader::new(File::open(&wrapped).unwrap(), None)
            .unwrap();
    let mut standard = zip::ZipArchive::new(inner).unwrap();
    assert_eq!(standard.len(), rules.manifest().objects.len() + 1);
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
}

#[test]
fn forwarding_preserves_exact_inner_zip_and_uses_a_separate_new_output_password() {
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(root.path());
    let flag = AtomicBool::new(false);
    let bare = root.path().join("original.tmcase.zip");
    write_case_package(&prepared, &bare, PackageFormat::BareZip, None, false, &flag).unwrap();
    let original = std::fs::read(&bare).unwrap();
    let package = Package::open(&bare, None, &flag).unwrap();
    let path = root.path().join("protected");
    let first = CapturePassword::new("first-output".into());
    package
        .write_transfer(&path, PackageFormat::Native, Some(&first), false, &flag)
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
        .write_transfer(
            &forwarded,
            PackageFormat::Native,
            Some(&second),
            true,
            &flag,
        )
        .unwrap();
    assert_ne!(std::fs::read(&forwarded).unwrap(), before);
    assert!(Package::open(&forwarded, Some(&first), &flag).is_err());
    let second_package = Package::open(&forwarded, Some(&second), &flag).unwrap();
    assert_eq!(
        second_package.manifest().encode().unwrap(),
        protected.manifest().encode().unwrap()
    );
    let copied = root.path().join("explicit-readable.tmcase.zip");
    second_package
        .write_transfer(&copied, PackageFormat::BareZip, None, false, &flag)
        .unwrap();
    assert_eq!(std::fs::read(copied).unwrap(), original);
    assert_eq!(std::fs::read(&bare).unwrap(), original);
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
    assert!(
        write_case_package(&prepared, &target, PackageFormat::Native, None, true, &flag).is_err()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"existing evidence");
    assert!(!std::fs::read_dir(root.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".case-transfer-")
    }));
    drop(held);
    write_case_package(&prepared, &target, PackageFormat::Native, None, true, &flag).unwrap();
    Package::open(&target, None, &flag).unwrap();
}
