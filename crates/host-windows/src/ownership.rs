use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::WindowsHostError;

/// Coordinates all journals that change the same current-user proxy registry.
#[derive(Clone, Debug)]
pub(crate) struct SharedProxyOwner {
    root: Arc<OnceLock<PathBuf>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRecord {
    schema: u32,
    journal: PathBuf,
}

impl SharedProxyOwner {
    #[cfg(test)]
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(OnceLock::from(root)),
        }
    }

    pub(crate) fn current_user() -> Self {
        Self {
            root: Arc::new(OnceLock::new()),
        }
    }

    fn root(&self) -> Result<&Path, WindowsHostError> {
        if let Some(root) = self.root.get() {
            return Ok(root);
        }
        // HKCU proxy settings are shared even when a caller overrides
        // LOCALAPPDATA for an isolated product profile. Use the OS known folder.
        let json = crate::run_script_bounded(
            "[Environment]::GetFolderPath('LocalApplicationData') | ConvertTo-Json -Compress",
            None,
            std::time::Duration::from_secs(10),
        )?;
        let root: String = serde_json::from_str(&json).map_err(WindowsHostError::Journal)?;
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err(WindowsHostError::InvalidJournal);
        }
        let _ = self.root.set(root.join("Transmog"));
        self.root
            .get()
            .map(PathBuf::as_path)
            .ok_or(WindowsHostError::InvalidJournal)
    }

    pub(crate) fn lease(&self) -> Result<File, WindowsHostError> {
        lock_file(&self.root()?.join("proxy-owner-v1.lock"))
    }

    pub(crate) fn recovery_pending(&self) -> bool {
        // Status polling must not launch PowerShell. Recovery initializes this
        // cache before reading the owner record; retain retry visibility then.
        let Some(root) = self.root.get() else {
            return false;
        };
        if !root
            .join("proxy-owner-v1.json")
            .try_exists()
            .unwrap_or(true)
        {
            return false;
        }
        // A live capture in another profile is not abandoned recovery state.
        // Probe without creating or modifying files; uncertainty stays pending.
        let Ok(lease) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("proxy-owner-v1.lock"))
        else {
            return true;
        };
        !matches!(lease.try_lock(), Err(fs::TryLockError::WouldBlock))
    }

    pub(crate) fn pending(&self) -> Result<Option<PathBuf>, WindowsHostError> {
        let record: Option<OwnerRecord> = read_json(&self.root()?.join("proxy-owner-v1.json"))?;
        match record {
            Some(record) if record.schema == 1 && record.journal.is_absolute() => {
                Ok(Some(record.journal))
            }
            Some(_) => Err(WindowsHostError::InvalidJournal),
            None => Ok(None),
        }
    }

    pub(crate) fn claim(&self, journal: &Path) -> Result<(), WindowsHostError> {
        write_new_json(
            &self.root()?.join("proxy-owner-v1.json"),
            &OwnerRecord {
                schema: 1,
                journal: std::path::absolute(journal).map_err(WindowsHostError::Io)?,
            },
        )
    }

    pub(crate) fn clear(&self, journal: &Path) -> Result<(), WindowsHostError> {
        if let Some(pending) = self.pending()? {
            if !same_path(&pending, journal) {
                return Err(WindowsHostError::InvalidJournal);
            }
            fs::remove_file(self.root()?.join("proxy-owner-v1.json"))
                .map_err(WindowsHostError::Io)?;
        }
        Ok(())
    }
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    // Cleanup compares ownership after unlinking the snapshot. Canonicalize the
    // still-existing parent so junction/alias paths retain the same identity.
    let identity = |path: &Path| {
        fs::canonicalize(path)
            .or_else(|_| match path.parent().zip(path.file_name()) {
                Some((parent, name)) => fs::canonicalize(parent).map(|parent| parent.join(name)),
                None => std::path::absolute(path),
            })
            .or_else(|_| std::path::absolute(path))
    };
    let left = identity(left);
    let right = identity(right);
    match (left, right) {
        (Ok(left), Ok(right)) => {
            left == right
                || (cfg!(windows) && left.as_os_str().eq_ignore_ascii_case(right.as_os_str()))
        }
        _ => false,
    }
}

pub(crate) fn lock_file(path: &Path) -> Result<File, WindowsHostError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(WindowsHostError::Io)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(WindowsHostError::Io)?;
    file.try_lock().map_err(|error| match error {
        fs::TryLockError::WouldBlock => WindowsHostError::ProxyInUse,
        fs::TryLockError::Error(error) => WindowsHostError::Io(error),
    })?;
    Ok(file)
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, WindowsHostError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(WindowsHostError::Io(error)),
    };
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(WindowsHostError::Io)?;
    if bytes.len() > 64 * 1024 {
        return Err(WindowsHostError::InvalidJournal);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(WindowsHostError::Journal)
}

pub(crate) fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), WindowsHostError> {
    write_json(path, value, false)
}

pub(crate) fn replace_json(path: &Path, value: &impl Serialize) -> Result<(), WindowsHostError> {
    write_json(path, value, true)
}

fn write_json(path: &Path, value: &impl Serialize, replace: bool) -> Result<(), WindowsHostError> {
    let parent = path.parent().ok_or(WindowsHostError::InvalidJournal)?;
    fs::create_dir_all(parent).map_err(WindowsHostError::Io)?;
    let bytes = serde_json::to_vec(value).map_err(WindowsHostError::Journal)?;
    // Never apply proxy settings after saving a snapshot the reader would reject.
    if bytes.len() > 64 * 1024 {
        return Err(WindowsHostError::InvalidJournal);
    }
    let mut file = tempfile::Builder::new()
        .prefix(".transmog-proxy-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(WindowsHostError::Io)?;
    file.write_all(&bytes).map_err(WindowsHostError::Io)?;
    file.as_file().sync_all().map_err(WindowsHostError::Io)?;
    if replace {
        file.persist(path)
            .map_err(|error| WindowsHostError::Io(error.error))?;
    } else {
        file.persist_noclobber(path)
            .map_err(|error| WindowsHostError::Io(error.error))?;
    }
    Ok(())
}
