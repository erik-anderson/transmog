use super::*;
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default)]
struct MemoryProtection {
    next: AtomicU64,
    secrets: Mutex<HashMap<u64, Vec<u8>>>,
}
impl KeyProtection for MemoryProtection {
    fn protect(&self, bytes: &[u8]) -> io::Result<Vec<u8>> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.secrets.lock().unwrap().insert(id, bytes.to_vec());
        Ok(envelope(255, &id.to_le_bytes()))
    }
    fn unprotect(&self, bytes: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        let id = u64::from_le_bytes(payload(bytes, 255)?.try_into().map_err(|_| invalid())?);
        self.secrets
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .map(Zeroizing::new)
            .ok_or_else(unavailable)
    }
    fn forget(&self, bytes: &[u8]) -> io::Result<()> {
        let id = u64::from_le_bytes(payload(bytes, 255)?.try_into().map_err(|_| invalid())?);
        self.secrets.lock().unwrap().remove(&id);
        Ok(())
    }
}

struct Unavailable;
impl KeyProtection for Unavailable {
    fn protect(&self, _: &[u8]) -> io::Result<Vec<u8>> {
        Err(unavailable())
    }
    fn unprotect(&self, _: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        Err(unavailable())
    }
    fn forget(&self, _: &[u8]) -> io::Result<()> {
        Err(unavailable())
    }
}

#[test]
fn publication_and_migration_never_leave_plaintext_or_overwrite_a_destination() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("ca.key");
    let protection = MemoryProtection::default();
    let key = b"-----BEGIN PRIVATE KEY-----\nsecret fixture\n";
    write_new(&path, key, &protection).unwrap();
    let protected = std::fs::read(&path).unwrap();
    assert!(is_protected(&protected));
    assert!(!protected.windows(key.len()).any(|bytes| bytes == key));
    assert_eq!(&*read(&path, &protection).unwrap(), key);
    assert!(write_new(&path, b"replacement", &protection).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), protected);
    assert_eq!(protection.secrets.lock().unwrap().len(), 1);
    remove(&path, &protection).unwrap();
    assert!(protection.secrets.lock().unwrap().is_empty());
    std::fs::write(&path, key).unwrap();
    assert!(protect_existing(&path, &Unavailable).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), key);
    protect_existing(&path, &protection).unwrap();
    let protected = std::fs::read(&path).unwrap();
    protect_existing(&path, &protection).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), protected);
    assert_eq!(&*read(&path, &protection).unwrap(), key);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn protection_failure_preserves_files_and_never_falls_back_to_plaintext() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("ca.key");
    assert!(write_new(&path, b"secret fixture", &Unavailable).is_err());
    assert!(!path.exists());
    let protection = MemoryProtection::default();
    write_new(&path, b"secret fixture", &protection).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(read(&path, &Unavailable).is_err());
    assert!(protect_existing(&path, &Unavailable).is_err());
    assert!(remove(&path, &Unavailable).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn rejects_non_files_and_bounded_input() {
    let root = tempfile::tempdir().unwrap();
    assert!(read(root.path(), &Unavailable).is_err());
    let path = root.path().join("large.key");
    File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
    assert!(read(&path, &Unavailable).is_err());
}

#[cfg(windows)]
#[test]
fn dpapi_round_trip_tamper_detection_and_version_platform_rejection() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("root.key");
    let plaintext = b"-----BEGIN PRIVATE KEY-----\nDPAPI fixture only\n";
    write_new(&path, plaintext, &SystemKeyProtection).unwrap();
    let protected = std::fs::read(&path).unwrap();
    assert!(is_protected(&protected));
    assert!(
        !protected
            .windows(plaintext.len())
            .any(|bytes| bytes == plaintext)
    );
    assert_eq!(&*read(&path, &SystemKeyProtection).unwrap(), plaintext);
    // A copy works for this user without any dependence on its pathname.
    let copy = root.path().join("copy.key");
    std::fs::write(&copy, &protected).unwrap();
    assert_eq!(&*read(&copy, &SystemKeyProtection).unwrap(), plaintext);
    for offset in [0, MAGIC.len(), MAGIC.len() + 1, protected.len() - 1] {
        let mut corrupt = protected.clone();
        corrupt[offset] ^= 0x7f;
        std::fs::write(&path, &corrupt).unwrap();
        assert!(read(&path, &SystemKeyProtection).is_err());
        assert!(protect_existing(&path, &SystemKeyProtection).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
#[ignore = "Requires an unlocked native Keychain or desktop Secret Service"]
fn native_keyring_round_trip_and_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("ca.key");
    write_new(&path, b"native fixture only", &SystemKeyProtection).unwrap();
    assert_eq!(
        &*read(&path, &SystemKeyProtection).unwrap(),
        b"native fixture only"
    );
    let protected = std::fs::read(&path).unwrap();
    remove(&path, &SystemKeyProtection).unwrap();
    assert!(SystemKeyProtection.unprotect(&protected).is_err());
}
