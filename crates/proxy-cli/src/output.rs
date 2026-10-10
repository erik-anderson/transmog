//! Prepare destinations without overwriting files; publish only complete output.

use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub(crate) struct PendingOutput {
    file: NamedTempFile,
    destination: PathBuf,
}

pub(crate) struct PublishError {
    pub(crate) error: io::Error,
    pub(crate) file: NamedTempFile,
}

impl PendingOutput {
    pub(crate) fn new(destination: &Path) -> io::Result<Self> {
        // symlink_metadata also rejects an existing dangling symlink.
        match std::fs::symlink_metadata(destination) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "Destination already exists: {}. Choose a new filename",
                        destination.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file = tempfile::Builder::new()
            .prefix(".transmog-")
            .suffix(".tmcap")
            .tempfile_in(parent)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "Cannot prepare destination {}: {error}",
                        destination.display()
                    ),
                )
            })?;
        Ok(Self {
            file,
            destination: destination.to_path_buf(),
        })
    }

    pub(crate) fn file(&mut self) -> &mut File {
        self.file.as_file_mut()
    }

    pub(crate) fn preserve(self) -> PathBuf {
        preserve(self.file)
    }

    pub(crate) fn publish(self) -> Result<(), PublishError> {
        if let Err(error) = self.file.as_file().sync_all() {
            return Err(PublishError {
                error,
                file: self.file,
            });
        }
        self.file
            .persist_noclobber(&self.destination)
            .map(|_| ())
            .map_err(|error| PublishError {
                error: error.error,
                file: error.file,
            })
    }
}

pub(crate) fn preserve(mut file: NamedTempFile) -> PathBuf {
    // Even a failed sync or Windows temporary-attribute change must not delete
    // available evidence when the error or file handle is dropped.
    file.disable_cleanup(true);
    let path = file.path().to_path_buf();
    let _ = file.as_file().sync_all();
    file.keep().map_or(path, |(_, path)| path)
}

pub(crate) fn write_new<T>(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let mut pending = PendingOutput::new(path)?;
    let result = write(pending.file())?;
    pending.publish().map_err(|error| error.error)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn failed_writes_leave_no_destination_and_allow_retry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.saz");
        let result = write_new(&path, |file| {
            file.write_all(b"partial")?;
            Err::<(), _>(io::Error::other("conversion failed").into())
        });
        assert!(result.is_err());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        write_new(&path, |file| Ok(file.write_all(b"complete")?)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"complete");
    }

    #[test]
    fn competing_destination_is_preserved_and_capture_remains_recoverable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.tmcap");
        let mut pending = PendingOutput::new(&path).unwrap();
        pending.file().write_all(b"captured evidence").unwrap();
        std::fs::write(&path, b"other file").unwrap();
        let error = pending.publish().err().unwrap();
        let recovery = preserve(error.file);
        assert_eq!(std::fs::read(&path).unwrap(), b"other file");
        assert_eq!(std::fs::read(recovery).unwrap(), b"captured evidence");
    }
}
