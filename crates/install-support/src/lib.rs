//! Installation-scoped replacement. Retired running executables never block a new payload.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
};

#[cfg(test)]
mod tests;

const BINARIES: [&str; 4] = [
    "transmog.exe",
    "transmog-cli.exe",
    "transmog-script-host.exe",
    "transmog-preview-worker.exe",
];
const RETIRED: &str = ".transmog-retired";
const GENERATION: &str = ".transmog-installation.json";
const LOCK: &str = ".transmog-update.lock";
const JOURNAL: &str = "transaction.json";
const MAX_RECORD: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    previous: Option<Vec<u8>>,
    replacement: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum State {
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    state: State,
    entries: Vec<Entry>,
}

/// Bounded cleanup evidence; locked files remain eligible on a later start.
#[derive(Debug, Default)]
pub struct Cleanup {
    /// Successfully deleted retired files.
    pub deleted: usize,
    /// Retired files or records which could not safely be removed.
    pub deferred: usize,
    /// Replacement succeeded, but the optional connector shutdown hint could not be published.
    pub notification_failed: bool,
}

fn invalid() -> io::Error {
    io::Error::other("The installation transaction is invalid or unavailable")
}

fn ordinary(path: &Path, directory: bool) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
    }
    if meta.is_symlink() || meta.is_dir() != directory || (!directory && !meta.is_file()) {
        return Err(invalid());
    }
    Ok(())
}

fn directory(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    ordinary(path, true)
}

fn lease(root: &Path) -> io::Result<File> {
    lock_file(&root.join(LOCK), false)
}

fn shared_lease(root: &Path) -> io::Result<File> {
    lock_file(&root.join(LOCK), true)
}

fn lock_file(path: &Path, shared: bool) -> io::Result<File> {
    if path.try_exists()? {
        ordinary(path, false)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let result = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    result.map_err(|error| match error {
        fs::TryLockError::WouldBlock => {
            io::Error::new(io::ErrorKind::WouldBlock, "An installation is in progress")
        }
        fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

/// Holds a shared installation lease for the desktop's lifetime.
/// # Errors
/// Returns busy during replacement, or a repair error for a pending transaction.
/// Unmanaged portable/development directories require no writable installation state.
pub fn desktop_guard(executable: &Path) -> io::Result<Option<File>> {
    let root = executable.parent().ok_or_else(invalid)?;
    if !root.join(LOCK).exists() {
        return Ok(None);
    }
    let guard = shared_lease(root)?;
    for path in transactions(root, usize::MAX)? {
        if path.join(JOURNAL).try_exists()?
            && read_transaction(&path.join(JOURNAL))?.state == State::Prepared
        {
            return Err(io::Error::other(
                "An interrupted Transmog update needs recovery. Run the installer again before starting Transmog.",
            ));
        }
    }
    Ok(Some(guard))
}

fn digest(path: &Path) -> io::Result<Vec<u8>> {
    ordinary(path, false)?;
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash.finalize().to_vec())
}

fn atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(invalid)?)?;
    serde_json::to_writer(&mut file, value).map_err(|_| invalid())?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn read_transaction(path: &Path) -> io::Result<Transaction> {
    ordinary(path, false)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_RECORD + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(invalid());
    }
    let tx: Transaction = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if tx.entries.len() != BINARIES.len() {
        return Err(invalid());
    }
    for (entry, name) in tx.entries.iter().zip(BINARIES) {
        if entry.name != name
            || entry.replacement.len() != 32
            || entry.previous.as_ref().is_some_and(|hash| hash.len() != 32)
        {
            return Err(invalid());
        }
    }
    Ok(tx)
}

fn transactions(root: &Path, limit: usize) -> io::Result<Vec<PathBuf>> {
    let retired = root.join(RETIRED);
    if !retired.try_exists()? {
        return Ok(Vec::new());
    }
    ordinary(&retired, true)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(retired)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.strip_prefix("transaction-").is_some_and(|suffix| {
            suffix.len() == 12 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        }) {
            ordinary(&entry.path(), true)?;
            paths.push(entry.path());
            if paths.len() == limit {
                break;
            }
        }
    }
    Ok(paths)
}

fn abandoned_empty(path: &Path) -> io::Result<bool> {
    if path.join(JOURNAL).try_exists()? {
        return Ok(false);
    }
    for folder in ["old", "new"] {
        let directory = path.join(folder);
        if directory.try_exists()? {
            ordinary(&directory, true)?;
            if fs::read_dir(directory)?.next().is_some() {
                return Ok(false);
            }
        }
    }
    for folder in ["old", "new"] {
        let directory = path.join(folder);
        if directory.try_exists()? {
            fs::remove_dir(directory)?;
        }
    }
    fs::remove_dir(path)?;
    Ok(true)
}

fn rollback(root: &Path, path: &Path, tx: &Transaction) -> io::Result<()> {
    if path.join("old").try_exists()? {
        ordinary(&path.join("old"), true)?;
    }
    for entry in tx.entries.iter().rev() {
        let old = path.join("old").join(&entry.name);
        let installed = root.join(&entry.name);
        if old.try_exists()? {
            if entry.previous.as_ref() != Some(&digest(&old)?) {
                return Err(invalid());
            }
            if installed.try_exists()? {
                if digest(&installed)? != entry.replacement {
                    return Err(invalid());
                }
                discard_replacement(path, &installed, &entry.name)?;
            }
            fs::rename(old, installed)?;
        } else if let Some(previous) = &entry.previous {
            // A previous rollback may have restored this entry and then crashed
            // while removing its now-empty directories. Never mistake missing
            // rollback bytes and a mismatched installed file for recovery.
            if digest(&installed)? != *previous {
                return Err(invalid());
            }
        } else if installed.try_exists()? {
            if digest(&installed)? != entry.replacement {
                return Err(invalid());
            }
            discard_replacement(path, &installed, &entry.name)?;
        }
    }
    Ok(())
}

fn discard_replacement(transaction: &Path, installed: &Path, name: &str) -> io::Result<()> {
    // A client may have launched the new EXE before commit failed. Rollback
    // must move it aside too: a mapped executable cannot be deleted on Windows.
    let staged = transaction.join("new");
    directory(&staged)?;
    let discarded = staged.join(name);
    if discarded.try_exists()? {
        ordinary(&discarded, false)?;
        fs::remove_file(&discarded)?;
    }
    fs::rename(installed, discarded)
}

fn remove_transaction(path: &Path, tx: &Transaction, result: &mut Cleanup) -> io::Result<()> {
    for folder in ["old", "new"] {
        let directory = path.join(folder);
        if !directory.try_exists()? {
            continue;
        }
        ordinary(&directory, true)?;
        for entry in &tx.entries {
            let file = directory.join(&entry.name);
            if !file.try_exists()? {
                continue;
            }
            let expected = if folder == "old" {
                entry.previous.as_ref()
            } else {
                Some(&entry.replacement)
            };
            ordinary(&file, false)?;
            // A crash may leave a partially copied staged replacement. Backup
            // bytes, unlike staging bytes, must retain their recorded identity.
            if folder == "old" && expected != Some(&digest(&file)?) {
                return Err(invalid());
            }
            if fs::remove_file(file).is_ok() {
                result.deleted += 1;
            } else {
                result.deferred += 1;
            }
        }
    }
    for folder in ["old", "new"] {
        let directory = path.join(folder);
        if directory.try_exists()? && fs::read_dir(&directory)?.next().is_none() {
            fs::remove_dir(directory)?;
        }
    }
    if !path.join("old").try_exists()? && !path.join("new").try_exists()? {
        if fs::read_dir(path)?.any(|entry| entry.is_ok_and(|entry| entry.file_name() != JOURNAL)) {
            return Err(invalid());
        }
        fs::remove_file(path.join(JOURNAL))?;
        fs::remove_dir(path)?;
    }
    Ok(())
}

/// Retries only completed transactions; never deletes pending rollback copies.
/// # Errors
/// Returns unavailable or busy installation state. Callers must treat cleanup as best effort.
pub fn cleanup(root: &Path) -> io::Result<Cleanup> {
    ordinary(root, true)?;
    if !root.join(RETIRED).try_exists()? {
        return Ok(Cleanup::default());
    }
    ordinary(&root.join(RETIRED), true)?;
    let _lease = shared_lease(root)?;
    let _cleanup = lock_file(&root.join(RETIRED).join(".cleanup.lock"), false)?;
    let mut result = Cleanup::default();
    for path in transactions(root, 128)? {
        if abandoned_empty(&path).unwrap_or(false) {
            continue;
        }
        match read_transaction(&path.join(JOURNAL)) {
            Ok(tx) if tx.state != State::Prepared => {
                if remove_transaction(&path, &tx, &mut result).is_err() {
                    result.deferred += 1;
                }
            }
            Ok(_) => {}
            Err(_) => result.deferred += 1,
        }
    }
    Ok(result)
}

/// Replaces the exact bundled executable set, rolling back an unsuccessful attempt.
/// The caller extracts these files from the verified installer into a private staging directory.
/// # Errors
/// Returns a staging, move, verification or recovery failure; never reports a partial update as success.
pub fn apply(root: &Path, payload: &Path) -> io::Result<Cleanup> {
    if !root.is_absolute() || root.parent().is_none() {
        return Err(invalid());
    }
    fs::create_dir_all(root)?;
    ordinary(root, true)?;
    ordinary(payload, true)?;
    let _lease = lease(root)?;
    directory(&root.join(RETIRED))?;
    // An update must inspect every pending transaction, even though background
    // cleanup limits the amount of work performed on any one app start.
    let mut result = Cleanup::default();
    for path in transactions(root, usize::MAX)? {
        if abandoned_empty(&path)? {
            continue;
        }
        let mut tx = read_transaction(&path.join(JOURNAL))?;
        if tx.state == State::Prepared {
            rollback(root, &path, &tx)?;
            tx.state = State::RolledBack;
            atomic_json(&path.join(JOURNAL), &tx)?;
        }
        if remove_transaction(&path, &tx, &mut result).is_err() {
            result.deferred += 1;
        }
    }
    let mut tx = Transaction {
        state: State::Prepared,
        entries: Vec::new(),
    };
    for name in BINARIES {
        let source = payload.join(name);
        let replacement = digest(&source)?;
        let installed = root.join(name);
        let previous = if installed.try_exists()? {
            Some(digest(&installed)?)
        } else {
            None
        };
        tx.entries.push(Entry {
            name: name.into(),
            previous,
            replacement,
        });
    }
    let path = tempfile::Builder::new()
        .prefix("transaction-")
        .rand_bytes(12)
        .tempdir_in(root.join(RETIRED))?
        .keep();
    directory(&path.join("old"))?;
    directory(&path.join("new"))?;
    atomic_json(&path.join(JOURNAL), &tx)?;
    let install = (|| {
        for entry in &tx.entries {
            let staged = path.join("new").join(&entry.name);
            fs::copy(payload.join(&entry.name), &staged)?;
            OpenOptions::new().write(true).open(&staged)?.sync_all()?;
            if digest(&staged)? != entry.replacement {
                return Err(invalid());
            }
        }
        for entry in &tx.entries {
            if entry.previous.is_some() {
                if entry.previous.as_ref() != Some(&digest(&root.join(&entry.name))?) {
                    return Err(invalid());
                }
                fs::rename(root.join(&entry.name), path.join("old").join(&entry.name))?;
            }
        }
        for entry in &tx.entries {
            fs::rename(path.join("new").join(&entry.name), root.join(&entry.name))?;
            if digest(&root.join(&entry.name))? != entry.replacement {
                return Err(invalid());
            }
        }
        // Publish the commit before signalling connectors. A stale signal cannot
        // make a failed installation look like a new executable generation.
        tx.state = State::Committed;
        atomic_json(&path.join(JOURNAL), &tx)?;
        Ok(())
    })();
    if let Err(error) = install {
        rollback(root, &path, &tx)?;
        tx.state = State::RolledBack;
        atomic_json(&path.join(JOURNAL), &tx)?;
        return Err(error);
    }
    if atomic_json(
        &root.join(GENERATION),
        &path.file_name().ok_or_else(invalid)?.to_string_lossy(),
    )
    .is_err()
    {
        result.notification_failed = true;
    }
    if remove_transaction(&path, &tx, &mut result).is_err() {
        result.deferred += 1;
    }
    Ok(result)
}

/// Captured installation generation for an MCP connector, including offline sessions.
pub struct InstallationWatch {
    root: PathBuf,
    generation: Option<Vec<u8>>,
}

impl InstallationWatch {
    /// Captures the logical installation directory before its executable is moved.
    /// # Errors
    /// Returns busy while an installer holds the installation lease.
    pub fn capture(executable: &Path) -> io::Result<Self> {
        let root = executable.parent().ok_or_else(invalid)?.to_owned();
        // Development/portable directories without installation metadata need no
        // writable state merely to initialize an offline connector.
        let _guard = desktop_guard(executable)?;
        let generation = read_generation(&root)?;
        Ok(Self { root, generation })
    }

    /// A committed replacement is a shutdown hint, independent of desktop activity.
    pub fn replaced(&self) -> bool {
        read_generation(&self.root).is_ok_and(|generation| generation != self.generation)
    }
}

fn read_generation(root: &Path) -> io::Result<Option<Vec<u8>>> {
    let path = root.join(GENERATION);
    if !path.try_exists()? {
        return Ok(None);
    }
    ordinary(&path, false)?;
    let mut bytes = Vec::new();
    File::open(path)?.take(257).read_to_end(&mut bytes)?;
    if bytes.len() > 256 {
        return Err(invalid());
    }
    Ok(Some(bytes))
}
