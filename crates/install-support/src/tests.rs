use super::*;

fn payload(root: &Path, prefix: &str) {
    fs::create_dir_all(root).unwrap();
    for name in BINARIES {
        fs::write(root.join(name), format!("{prefix}:{name}")).unwrap();
    }
}

fn assert_payload(root: &Path, prefix: &str) {
    for name in BINARIES {
        assert_eq!(
            fs::read_to_string(root.join(name)).unwrap(),
            format!("{prefix}:{name}")
        );
    }
}

fn fixture_transaction(root: &Path, sources: &Path, committed: bool) -> (PathBuf, Transaction) {
    let path = root.join(RETIRED).join("transaction-123456abcdef");
    fs::create_dir_all(path.join("old")).unwrap();
    fs::create_dir_all(path.join("new")).unwrap();
    let tx = Transaction {
        state: if committed {
            State::Committed
        } else {
            State::Prepared
        },
        entries: BINARIES
            .into_iter()
            .map(|name| Entry {
                name: name.into(),
                previous: Some(digest(&root.join(name)).unwrap()),
                replacement: digest(&sources.join(name)).unwrap(),
            })
            .collect(),
    };
    atomic_json(&path.join(JOURNAL), &tx).unwrap();
    (path, tx)
}

#[test]
fn replacement_signals_offline_connectors_and_cleans_known_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    fs::write(root.join("user.tmcap"), "preserve").unwrap();
    let watch = InstallationWatch::capture(&root.join("transmog-cli.exe")).unwrap();
    assert!(!watch.replaced());
    let result = apply(&root, &source).unwrap();
    assert_eq!(result.deferred, 0);
    assert_eq!(result.deleted, 4);
    assert_payload(&root, "new");
    assert!(watch.replaced());
    assert!(
        !InstallationWatch::capture(&root.join("transmog-cli.exe"))
            .unwrap()
            .replaced()
    );
    assert_eq!(
        fs::read_to_string(root.join("user.tmcap")).unwrap(),
        "preserve"
    );
    assert_eq!(transactions(&root, usize::MAX).unwrap().len(), 0);
}

#[test]
fn fresh_install_creates_a_complete_launchable_binary_set() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("new-installation");
    let source = temp.path().join("payload");
    payload(&source, "new");
    assert_eq!(apply(&root, &source).unwrap().deferred, 0);
    assert_payload(&root, "new");
    assert!(desktop_guard(&root.join("transmog.exe")).unwrap().is_some());
    assert!(InstallationWatch::capture(&root.join("transmog-cli.exe")).is_ok());
}

#[test]
fn incomplete_payload_cannot_change_installed_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    fs::remove_file(source.join("transmog-cli.exe")).unwrap();
    assert!(apply(&root, &source).is_err());
    assert_payload(&root, "old");
    assert!(!root.join(GENERATION).exists());
}

#[test]
fn next_install_recovers_an_interrupted_partial_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    let (path, _) = fixture_transaction(&root, &source, false);
    drop(lease(&root).unwrap());
    assert!(InstallationWatch::capture(&root.join("transmog-cli.exe")).is_err());
    for name in BINARIES.into_iter().take(2) {
        fs::rename(root.join(name), path.join("old").join(name)).unwrap();
    }
    fs::copy(source.join(BINARIES[0]), root.join(BINARIES[0])).unwrap();
    fs::write(path.join("new").join(BINARIES[3]), "partial staging bytes").unwrap();
    cleanup(&root).unwrap();
    assert!(
        path.join("old").join(BINARIES[0]).exists(),
        "Startup must retain pending rollback bytes"
    );
    apply(&root, &source).unwrap();
    assert_payload(&root, "new");
    assert_eq!(transactions(&root, usize::MAX).unwrap().len(), 0);
}

#[test]
fn cleanup_preserves_unknown_and_invalid_records() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("source");
    payload(&source, "new");
    payload(root, "old");
    let (path, mut tx) = fixture_transaction(root, &source, true);
    tx.entries[0].name = "../user.tmcap".into();
    atomic_json(&path.join(JOURNAL), &tx).unwrap();
    fs::write(root.join("user.tmcap"), "preserve").unwrap();
    assert!(cleanup(root).unwrap().deferred > 0);
    assert_eq!(
        fs::read_to_string(root.join("user.tmcap")).unwrap(),
        "preserve"
    );
    assert_payload(root, "old");
}

#[test]
fn installation_lease_blocks_concurrent_update_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("source");
    payload(&source, "new");
    payload(&root, "old");
    fs::create_dir(root.join(RETIRED)).unwrap();
    let held = lease(&root).unwrap();
    assert_eq!(
        apply(&root, &source).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        cleanup(&root).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(InstallationWatch::capture(&root.join("transmog-cli.exe")).is_err());
    assert_payload(&root, "old");
    drop(held);
    apply(&root, &source).unwrap();
}

#[test]
fn desktop_lifetime_excludes_updates_but_allows_connectors_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    apply(&root, &source).unwrap();
    let held = desktop_guard(&root.join("transmog.exe")).unwrap();
    let watch = InstallationWatch::capture(&root.join("transmog-cli.exe")).unwrap();
    cleanup(&root).unwrap();
    assert_eq!(
        apply(&root, &source).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(held);
    apply(&root, &source).unwrap();
    assert!(watch.replaced());
}

#[cfg(windows)]
#[test]
fn failed_rename_rolls_back_and_never_signals_success() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("source");
    payload(&source, "new");
    payload(&root, "old");
    let held = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(root.join("transmog-cli.exe"))
        .unwrap();
    assert!(apply(&root, &source).is_err());
    assert_payload(&root, "old");
    assert!(!root.join(GENERATION).exists());
    drop(held);
    apply(&root, &source).unwrap();
    assert_payload(&root, "new");
}

#[cfg(windows)]
#[test]
fn startup_retries_locked_retired_files_after_their_process_releases_them() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("source");
    payload(&source, "new");
    payload(&root, "old");
    let (path, _) = fixture_transaction(&root, &source, true);
    let retired = path.join("old").join("transmog-cli.exe");
    fs::rename(root.join("transmog-cli.exe"), &retired).unwrap();
    fs::copy(
        source.join("transmog-cli.exe"),
        root.join("transmog-cli.exe"),
    )
    .unwrap();
    let held = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&retired)
        .unwrap();
    assert_eq!(cleanup(&root).unwrap().deferred, 1);
    assert!(retired.exists());
    assert!(
        apply(&root, &source).unwrap().deferred >= 1,
        "A retired file must not block a subsequent update"
    );
    drop(held);
    assert_eq!(cleanup(&root).unwrap().deleted, 1);
    assert!(!path.exists());
}

#[test]
fn rollback_cleanup_interruption_is_recoverable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    let (path, _) = fixture_transaction(&root, &source, false);
    fs::remove_dir(path.join("old")).unwrap();
    fs::remove_dir(path.join("new")).unwrap();
    apply(&root, &source).unwrap();
    assert_payload(&root, "new");
}

#[test]
fn rollback_retires_replacement_bytes_before_restoring_the_original() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    let (path, tx) = fixture_transaction(&root, &source, false);
    let name = "transmog-cli.exe";
    fs::rename(root.join(name), path.join("old").join(name)).unwrap();
    fs::copy(source.join(name), root.join(name)).unwrap();
    rollback(&root, &path, &tx).unwrap();
    assert_payload(&root, "old");
    assert_eq!(
        fs::read_to_string(path.join("new").join(name)).unwrap(),
        format!("new:{name}")
    );
}

#[test]
fn update_recovers_pending_transactions_beyond_the_startup_cleanup_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    for index in 0..128 {
        fs::create_dir_all(root.join(RETIRED).join(format!("transaction-{index:012}"))).unwrap();
    }
    let (path, _) = fixture_transaction(&root, &source, false);
    fs::rename(root.join(BINARIES[0]), path.join("old").join(BINARIES[0])).unwrap();
    apply(&root, &source).unwrap();
    assert_payload(&root, "new");
    assert_eq!(transactions(&root, usize::MAX).unwrap().len(), 0);
}

#[test]
fn unknown_retirement_files_survive_without_blocking_later_updates() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    let (path, _) = fixture_transaction(&root, &source, true);
    let unknown = path.join("user.tmcap");
    fs::write(&unknown, "preserve").unwrap();
    assert!(cleanup(&root).unwrap().deferred > 0);
    assert!(path.join(JOURNAL).exists());
    apply(&root, &source).unwrap();
    assert_eq!(fs::read_to_string(unknown).unwrap(), "preserve");
}

#[test]
fn failed_notification_does_not_misreport_a_committed_update_as_failed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("installed");
    let source = temp.path().join("payload");
    payload(&root, "old");
    payload(&source, "new");
    fs::create_dir(root.join(GENERATION)).unwrap();
    assert!(apply(&root, &source).unwrap().notification_failed);
    assert_payload(&root, "new");
}

#[cfg(unix)]
#[test]
fn linked_retirement_directories_are_never_traversed() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("precious"), "preserve").unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join(RETIRED)).unwrap();
    assert!(cleanup(temp.path()).is_err());
    assert!(outside.path().join("precious").exists());
}
