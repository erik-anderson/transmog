//! Explicit source packaging reads a bounded immutable snapshot and never starts a worker.
use super::{Error, Origin, Registry, directory::Directory, invalid};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use transmog_extensions::write_package;

/// A completed archive save; source files and installation state are unchanged.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagedExtension {
    /// User-selected destination.
    pub path: PathBuf,
    /// Exact packaged source/declaration revision.
    pub digest: String,
    /// Archive size, bounded by the package format's payload and metadata limits.
    pub bytes: u64,
}
impl Registry {
    /// Packages the current registered development folder, including disabled entries.
    /// # Errors
    /// Rejects unsafe sources, invalid metadata, source overwrite, non-tmext destinations and failed writes.
    /// Validation failure preserves an existing output. No source module is evaluated.
    pub fn package_directory(
        &self,
        id: &str,
        destination: &Path,
    ) -> Result<PackagedExtension, Error> {
        if !destination
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmext"))
        {
            return Err(invalid(
                "Choose a .tmext filename for the packaged extension",
            ));
        }
        let entry = self
            .status()
            .entries
            .into_iter()
            .find(|entry| {
                entry.catalog.manifest.id == id && entry.catalog.origin == Origin::Development
            })
            .ok_or_else(|| {
                invalid("Packaging requires a registered unpacked development folder")
            })?;
        let root = entry.directory.as_deref().ok_or(Error::AccessDenied)?;
        let source = Directory::open(root)?;
        let snapshot = source.snapshot()?;
        if snapshot.manifest.id != id {
            return Err(invalid("The development extension identity changed"));
        }
        let destination = std::path::absolute(destination)?;
        let parent = destination
            .parent()
            .ok_or_else(|| invalid("Choose an extension destination folder"))?;
        let canonical_parent = parent.canonicalize()?;
        let canonical_root = root.canonicalize()?;
        let target = canonical_parent.join(destination.file_name().ok_or(Error::AccessDenied)?);
        // Avoid replacing an inventoried source, including a differently cased spelling
        // on Windows. Unlisted build outputs inside the development folder are allowed.
        let target_key = path_key(&target);
        if snapshot
            .manifest
            .files
            .iter()
            .any(|file| path_key(&canonical_root.join(&file.path)) == target_key)
        {
            return Err(invalid(
                "The package destination would overwrite a declared source file",
            ));
        }
        let mut staged = tempfile::NamedTempFile::new_in(&canonical_parent)?;
        write_package(staged.as_file_mut(), &snapshot, &AtomicBool::new(false))?;
        staged.as_file_mut().sync_all()?;
        let bytes = staged.as_file().metadata()?.len();
        // All reads/validation and the complete ZIP64 write finish before atomic replacement.
        staged
            .persist(&target)
            .map_err(|error| Error::Io(error.error))?;
        Ok(PackagedExtension {
            path: destination,
            digest: snapshot.digest,
            bytes,
        })
    }
}
fn path_key(path: &Path) -> String {
    let path = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        path.replace('\\', "/").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path
    }
}
