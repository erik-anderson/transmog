use crate::{CaseManifest, Error, PreparedCase, decimal, invalid};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
#[path = "working_fs.rs"]
mod fs;

/// An explicit plaintext working directory and an independently owned read snapshot.
#[derive(Debug)]
pub struct WorkingCase {
    root: PathBuf,
    prepared: PreparedCase,
    token: String,
    history: Vec<String>,
    recovery_artifacts: usize,
}
impl WorkingCase {
    /// Opens and verifies current metadata/objects without changing the directory.
    /// # Errors
    /// Rejects incomplete directories, unsafe members, corrupt bytes and cancellation.
    pub fn open(path: &Path, staging: &Path, canceled: &AtomicBool) -> Result<Self, Error> {
        let root = fs::root(path)?;
        let mut recovery_artifacts = fs::layout(&root)?;
        let bytes = fs::bytes(&root.join("manifest.json"))?;
        let prepared = PreparedCase::prepare(
            &bytes,
            staging,
            |sha| Ok(Box::new(fs::open(&root.join("objects/sha256").join(sha))?)),
            canceled,
        )?;
        let history = history(&root, prepared.manifest())?;
        for entry in std::fs::read_dir(root.join("history"))? {
            let entry = entry?;
            let number = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("Invalid history revision"))?;
            if decimal(&number, false)? >= decimal(&prepared.manifest().revision, false)?
                || !entry.path().join("manifest.json").exists()
            {
                recovery_artifacts += 1;
            }
        }
        Ok(Self {
            root,
            prepared,
            token: crate::sha256(&bytes),
            history,
            recovery_artifacts,
        })
    }
    /// Creates a new directory; an existing destination is never overwritten.
    /// # Errors
    /// Failure/cancellation leaves no valid partial case and preserves existing paths.
    pub fn create(
        prepared: PreparedCase,
        path: &Path,
        canceled: &AtomicBool,
    ) -> Result<Self, Error> {
        crate::case_prepare::check_cancel(canceled)?;
        let destination = std::path::absolute(path)?;
        let path = destination.as_path();
        let parent = path
            .parent()
            .ok_or_else(|| invalid("Choose a case directory"))?;
        fs::root(parent)?;
        std::fs::create_dir(path)?;
        let result = (|| {
            std::fs::create_dir(path.join("objects"))?;
            std::fs::create_dir(path.join("objects/sha256"))?;
            std::fs::create_dir(path.join("history"))?;
            copy_objects(&prepared, path, canceled)?;
            let bytes = prepared.manifest().encode()?;
            let temporary = manifest_temp(path, &bytes)?;
            crate::case_prepare::check_cancel(canceled)?;
            temporary
                .persist_noclobber(path.join("manifest.json"))
                .map_err(|e| Error::Io(e.error))?;
            Ok(Self {
                root: destination.clone(),
                prepared,
                token: crate::sha256(&bytes),
                history: vec![],
                recovery_artifacts: 0,
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(path);
        } // Only this operation's create-new directory.
        result
    }
    /// Current verified snapshot survives source movement/deletion until dropped.
    pub fn prepared(&self) -> &PreparedCase {
        &self.prepared
    }
    /// Previously committed manifest revisions; the current revision is separate.
    pub fn history(&self) -> &[String] {
        &self.history
    }
    /// Opens a retained historical revision with the same full object verification as current.
    /// # Errors
    /// Rejects stale readers, unknown revisions and missing/corrupt historical objects.
    pub fn open_revision(
        &self,
        revision: &str,
        staging: &Path,
        canceled: &AtomicBool,
    ) -> Result<PreparedCase, Error> {
        self.current()?;
        if !self.history.contains(&revision.to_owned()) {
            return Err(invalid("Historical revision is not retained"));
        }
        let bytes = fs::bytes(
            &self
                .root
                .join("history")
                .join(revision)
                .join("manifest.json"),
        )?;
        let case = PreparedCase::prepare(
            &bytes,
            staging,
            |sha| {
                Ok(Box::new(fs::open(
                    &self.root.join("objects/sha256").join(sha),
                )?))
            },
            canceled,
        )?;
        if case.manifest().id != self.prepared.manifest().id || case.manifest().revision != revision
        {
            return Err(invalid("Historical identity changed"));
        }
        Ok(case)
    }
    /// Owned staging remnants are reported as recovery candidates, never loaded as current.
    pub const fn recovery_artifacts(&self) -> usize {
        self.recovery_artifacts
    }
    /// Commits the next revision with immutable objects and an atomic current manifest.
    /// # Errors
    /// Rejects stale writers, changed case identity, history limits, corruption or cancellation.
    pub fn save(&mut self, next: PreparedCase, canceled: &AtomicBool) -> Result<(), Error> {
        let encoded = next.manifest().encode()?;
        let _lock = fs::lock(&self.root)?;
        self.current()?;
        let previous = self.prepared.manifest();
        if next.manifest().id != previous.id
            || decimal(&next.manifest().revision, false)?
                != decimal(&previous.revision, false)?
                    .checked_add(1)
                    .ok_or_else(|| invalid("Case revision exhausted"))?
        {
            return Err(invalid(
                "Save requires the same case and the next exact revision",
            ));
        }
        let prior_history = history(&self.root, previous)?;
        if prior_history.len() >= 31 {
            return Err(invalid(
                "Retain at most 32 working revisions; explicitly prune older history",
            ));
        }
        copy_objects(&next, &self.root, canceled)?;
        let directory = self.root.join("history").join(&previous.revision);
        if !directory.exists() {
            std::fs::create_dir(&directory)?;
        }
        fs::plain(&directory, true)?;
        let prior = self.root.join("manifest.json");
        let history_file = directory.join("manifest.json");
        let bytes = fs::bytes(&prior)?;
        if history_file.exists() {
            if fs::bytes(&history_file)? != bytes {
                return Err(invalid("Conflicting interrupted history save"));
            }
        } else {
            manifest_temp(&directory, &bytes)?
                .persist_noclobber(&history_file)
                .map_err(|e| Error::Io(e.error))?;
        }
        let temporary = manifest_temp(&self.root, &encoded)?;
        crate::case_prepare::check_cancel(canceled)?;
        self.current()?;
        temporary
            .persist(self.root.join("manifest.json"))
            .map_err(|e| Error::Io(e.error))?;
        self.token = crate::sha256(&encoded);
        self.history = prior_history;
        self.history.push(previous.revision.clone());
        self.prepared = next;
        Ok(())
    }
    /// Explicitly removes selected historical manifests; objects are retained for recovery.
    /// # Errors
    /// Rejects stale writers or unknown/current revisions. I/O failure preserves the current case.
    pub fn prune_history(&mut self, revisions: &[String]) -> Result<(), Error> {
        let _lock = fs::lock(&self.root)?;
        self.current()?;
        let available = history(&self.root, self.prepared.manifest())?;
        let unique = revisions.iter().collect::<std::collections::BTreeSet<_>>();
        if unique.len() != revisions.len() || revisions.iter().any(|r| !available.contains(r)) {
            return Err(invalid("Choose distinct historical revisions"));
        }
        for revision in revisions {
            let directory = self.root.join("history").join(revision);
            let children = fs::children(&directory)?;
            if children.len() != 1 || children[0].file_name() != "manifest.json" {
                return Err(invalid(
                    "Historical revision contains recovery data; inspect it before pruning",
                ));
            }
            std::fs::remove_file(directory.join("manifest.json"))?;
            std::fs::remove_dir(directory)?;
        }
        self.history = history(&self.root, self.prepared.manifest())?;
        Ok(())
    }
    fn current(&self) -> Result<(), Error> {
        fs::root(&self.root)?;
        fs::layout(&self.root)?;
        if crate::sha256(&fs::bytes(&self.root.join("manifest.json"))?) != self.token {
            return Err(invalid("Case changed after opening; reopen before saving"));
        }
        Ok(())
    }
}
fn manifest_temp(parent: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, Error> {
    let mut file = tempfile::Builder::new()
        .prefix(".case-manifest-")
        .tempfile_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    Ok(file)
}
fn copy_objects(prepared: &PreparedCase, root: &Path, canceled: &AtomicBool) -> Result<(), Error> {
    for object in &prepared.manifest().objects {
        crate::case_prepare::check_cancel(canceled)?;
        let parent = root.join("objects/sha256");
        fs::plain(&parent, true)?;
        let path = parent.join(&object.sha256);
        if path.exists() {
            let input = fs::open(&path)?;
            // Rehash existing immutable bytes; a digest filename is not integrity authority.
            let mut sink = std::io::sink();
            crate::prepare::copy_object(Box::new(input), &mut sink, object, canceled)?;
        } else {
            let mut file = tempfile::Builder::new()
                .prefix(".case-object-")
                .tempfile_in(&parent)?;
            crate::prepare::copy_object(
                Box::new(prepared.open_object(&object.sha256)?),
                file.as_file_mut(),
                object,
                canceled,
            )?;
            file.as_file().sync_all()?;
            file.persist_noclobber(path)
                .map_err(|e| Error::Io(e.error))?;
        }
    }
    Ok(())
}
fn history(root: &Path, current: &CaseManifest) -> Result<Vec<String>, Error> {
    let mut result = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir(root.join("history"))? {
        let entry = entry?;
        fs::plain(&entry.path(), true)?;
        let revision = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("Invalid history revision"))?;
        let number = decimal(&revision, false)?;
        count += 1;
        if count > 1024 {
            return Err(invalid("Excessive working history/staging directories"));
        }
        let children = fs::children(&entry.path())?;
        for child in &children {
            fs::plain(&child.path(), false)?;
            let name = child.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| invalid("Invalid historical member name"))?;
            if name != "manifest.json" && !name.starts_with(".case-manifest-") {
                return Err(invalid("Unexpected historical member"));
            }
        }
        if children.is_empty() {
            continue;
        } // Interrupted prune or uncommitted history staging.
        // An interrupted save can leave the current revision's recovery directory
        // empty or partial. It is staging, never a committed historical revision.
        if number >= decimal(&current.revision, false)? {
            continue;
        }
        let bytes = fs::bytes(&entry.path().join("manifest.json"))?;
        let manifest = CaseManifest::parse(&bytes)?;
        if manifest.id != current.id
            || manifest.revision != revision
            || number > decimal(&current.revision, false)?
        {
            return Err(invalid("Invalid/uncommitted historical manifest"));
        }
        if number < decimal(&current.revision, false)? {
            result.push(revision);
        }
    }
    result.sort_by_key(|r| decimal(r, false).unwrap_or(0));
    if result.len() > 31 {
        return Err(invalid("Working history exceeds 32 committed revisions"));
    }
    Ok(result)
}
