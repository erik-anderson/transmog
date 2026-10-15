//! Bounded, owned file snapshots. External paths are never used by a response pump.
use super::{MAX_ASSET_BYTES, STREAM_CHUNK_BYTES, unavailable};
use crate::AppError;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::Path,
    sync::{Arc, Mutex, Weak},
};

const MAX_ACTIVE_BYTES: u64 = 2 * MAX_ASSET_BYTES;
const MAX_ACTIVE_FILES: usize = 128;

#[derive(Debug)]
pub(super) struct SnapshotPool {
    // One acquisition at a time bounds temporary input + encoded output to 2 GiB.
    pub preparation: Mutex<()>,
    files: Mutex<BTreeMap<String, Weak<Snapshot>>>,
    max_bytes: u64,
    max_files: usize,
}
impl Default for SnapshotPool {
    fn default() -> Self {
        Self {
            preparation: Mutex::default(),
            files: Mutex::default(),
            max_bytes: MAX_ACTIVE_BYTES,
            max_files: MAX_ACTIVE_FILES,
        }
    }
}

#[derive(Debug)]
pub(super) struct Snapshot {
    pub file: tempfile::NamedTempFile,
    pub bytes: u64,
    pub sha256: String,
}

impl SnapshotPool {
    pub fn admit(&self, snapshot: Snapshot) -> Result<Arc<Snapshot>, String> {
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        files.retain(|_, file| file.strong_count() > 0);
        if let Some(existing) = files.get(&snapshot.sha256).and_then(Weak::upgrade) {
            return Ok(existing);
        }
        let used: u64 = files
            .values()
            .filter_map(Weak::upgrade)
            .map(|file| file.bytes)
            .sum();
        if files.len() >= self.max_files || used.saturating_add(snapshot.bytes) > self.max_bytes {
            return Err("Response file snapshot limit reached. Wait for active responses to finish and retry.".into());
        }
        let snapshot = Arc::new(snapshot);
        files.insert(snapshot.sha256.clone(), Arc::downgrade(&snapshot));
        Ok(snapshot)
    }
}

pub(super) fn open(path: &Path) -> Result<File, AppError> {
    let mut options = OpenOptions::new();
    options.read(true);
    // Permit atomic replacement, but exclude in-place writers while reading.
    // An already-open writer fails this open instead of supplying mixed bytes.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1 | 4); // FILE_SHARE_READ | FILE_SHARE_DELETE
    }
    options.open(path).map_err(|_| unavailable("Response file is unavailable (missing, locked or unreadable). Finish saving it, restore access or choose another file."))
}

pub(super) fn acquire(
    mut file: File,
    root: &Path,
    limit: u64,
    canceled: impl Fn() -> bool,
) -> Result<Snapshot, AppError> {
    acquire_checked(&mut file, root, limit, canceled, || {})
}

pub(super) fn owned(mut file: tempfile::NamedTempFile, limit: u64) -> Result<Snapshot, AppError> {
    let bytes = file
        .as_file()
        .metadata()
        .map_err(|_| unavailable("Snapshot metadata unavailable"))?
        .len();
    if bytes > limit {
        return Err(unavailable(
            "Encoded response exceeds the snapshot byte limit",
        ));
    }
    file.rewind()
        .map_err(|_| unavailable("Snapshot could not be read"))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; STREAM_CHUNK_BYTES];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| unavailable("Snapshot could not be read"))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    file.rewind()
        .map_err(|_| unavailable("Snapshot could not be read"))?;
    Ok(Snapshot {
        file,
        bytes,
        sha256: format!("{:x}", hash.finalize()),
    })
}

fn acquire_checked(
    file: &mut File,
    root: &Path,
    limit: u64,
    canceled: impl Fn() -> bool,
    between_reads: impl FnOnce(),
) -> Result<Snapshot, AppError> {
    let before = file
        .metadata()
        .map_err(|_| unavailable("Response file metadata is unavailable"))?;
    if !before.is_file() || limit == 0 || before.len() > limit {
        return Err(unavailable(
            "Response file exceeds the snapshot byte limit; choose a smaller regular file.",
        ));
    }
    let mut output = tempfile::NamedTempFile::new_in(root)
        .map_err(|_| unavailable("Response snapshot storage is unavailable"))?;
    let mut buffer = vec![0; STREAM_CHUNK_BYTES];
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    loop {
        if canceled() {
            return Err(unavailable("Response snapshot preparation canceled"));
        }
        let count = file
            .read(&mut buffer)
            .map_err(|_| unavailable("Response file could not be read"))?;
        if count == 0 {
            break;
        }
        bytes = bytes.saturating_add(count as u64);
        if bytes > limit {
            return Err(unavailable(
                "Response file grew beyond the snapshot byte limit",
            ));
        }
        output
            .write_all(&buffer[..count])
            .map_err(|_| unavailable("Response snapshot could not be written"))?;
        hash.update(&buffer[..count]);
    }
    between_reads();
    file.rewind()
        .map_err(|_| unavailable("Response file could not be verified"))?;
    let mut verified = Sha256::new();
    let mut verified_bytes = 0u64;
    loop {
        if canceled() {
            return Err(unavailable("Response snapshot preparation canceled"));
        }
        let count = file
            .read(&mut buffer)
            .map_err(|_| unavailable("Response file could not be verified"))?;
        if count == 0 {
            break;
        }
        verified_bytes = verified_bytes.saturating_add(count as u64);
        if verified_bytes > limit {
            return Err(unavailable(
                "Response file grew beyond the snapshot byte limit",
            ));
        }
        verified.update(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|_| unavailable("Response file metadata is unavailable"))?;
    let digest = hash.finalize();
    if bytes != before.len()
        || verified_bytes != bytes
        || digest != verified.finalize()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(unavailable(
            "Response file changed during preparation. Finish saving the file and retry.",
        ));
    }
    output
        .rewind()
        .map_err(|_| unavailable("Response snapshot could not be read"))?;
    Ok(Snapshot {
        file: output,
        bytes,
        sha256: format!("{digest:x}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_admits_duplicates_and_recovers_after_consumers_release() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("external");
        let pool = SnapshotPool {
            max_bytes: 4,
            max_files: 2,
            ..SnapshotPool::default()
        };
        let read = || acquire(open(&path).unwrap(), root.path(), 64, || false).unwrap();
        std::fs::write(&path, b"aaaa").unwrap();
        let first = pool.admit(read()).unwrap();
        let duplicate = pool.admit(read()).unwrap();
        std::fs::write(&path, b"bbbb").unwrap();
        assert!(pool.admit(read()).unwrap_err().contains("snapshot limit"));
        drop(first);
        drop(duplicate);
        assert_eq!(pool.admit(read()).unwrap().bytes, 4);
        assert!(
            acquire(open(&path).unwrap(), root.path(), 64, || true)
                .unwrap_err()
                .message
                .contains("canceled")
        );
        let count_pool = SnapshotPool {
            max_bytes: 64,
            max_files: 1,
            ..SnapshotPool::default()
        };
        let active = count_pool.admit(read()).unwrap();
        std::fs::write(&path, b"cccc").unwrap();
        assert!(
            count_pool
                .admit(read())
                .unwrap_err()
                .contains("snapshot limit")
        );
        drop(active);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn windows_excludes_writers_and_keeps_atomic_replacements_separate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("external");
        std::fs::write(&path, b"old").unwrap();
        let writer = OpenOptions::new().write(true).open(&path).unwrap();
        assert!(open(&path).is_err());
        drop(writer);
        let reader = open(&path).unwrap();
        assert!(std::fs::write(&path, b"bad").is_err());
        let replacement = root.path().join("replacement");
        std::fs::write(&replacement, b"new").unwrap();
        std::fs::rename(replacement, &path).unwrap();
        let old = acquire(reader, root.path(), 64, || false).unwrap();
        let new = acquire(open(&path).unwrap(), root.path(), 64, || false).unwrap();
        assert_eq!(std::fs::read(old.file.path()).unwrap(), b"old");
        assert_eq!(std::fs::read(new.file.path()).unwrap(), b"new");
        assert_ne!(old.sha256, new.sha256);
    }

    #[test]
    fn rejects_same_length_in_place_change_and_bounds_growth() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("external");
        std::fs::write(&path, b"first").unwrap();
        let mut file = File::open(&path).unwrap();
        let error = acquire_checked(
            &mut file,
            root.path(),
            64,
            || false,
            || {
                std::fs::write(&path, b"other").unwrap();
            },
        )
        .unwrap_err();
        assert!(error.message.contains("changed during preparation"));
        file.rewind().unwrap();
        assert!(
            acquire_checked(
                &mut file,
                root.path(),
                5,
                || false,
                || {
                    std::fs::write(&path, b"larger").unwrap();
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn deduplicates_owned_bytes_and_releases_them_after_consumers() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("external");
        std::fs::write(&path, b"same").unwrap();
        let pool = SnapshotPool::default();
        let first = pool
            .admit(acquire(open(&path).unwrap(), root.path(), 64, || false).unwrap())
            .unwrap();
        let second = pool
            .admit(acquire(open(&path).unwrap(), root.path(), 64, || false).unwrap())
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(std::fs::read(first.file.path()).unwrap(), b"same");
        let owned = first.file.path().to_owned();
        drop(first);
        assert!(owned.exists());
        drop(second);
        assert!(!owned.exists());
    }
}
