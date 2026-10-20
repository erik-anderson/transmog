//! Bounded OS notifications for an explicitly active developer root. No periodic source reads.
use std::path::Path;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
use transmog_extensions::Error;

#[derive(Default)]
struct Changes {
    paths: BTreeSet<String>,
    dirty: bool,
    changed: Option<Instant>,
    attempted: Option<Instant>,
}
impl Changes {
    fn update(&mut self, paths: &[String]) {
        self.paths = paths
            .iter()
            .map(|path| path.to_ascii_lowercase())
            .chain(std::iter::once("manifest.json".into()))
            .collect();
    }
    #[cfg(windows)]
    fn relevant(&self, name: &str) -> bool {
        let name = name.replace('\\', "/").to_ascii_lowercase();
        if self.paths.contains(&name) {
            return true;
        }
        let prefix = name + "/";
        self.paths
            .range(prefix.clone()..)
            .next()
            .is_some_and(|path| path.starts_with(&prefix))
    }
    #[cfg(windows)]
    fn mark(&mut self) {
        self.dirty = true;
        self.changed = Some(Instant::now());
    }
    fn ready(&self) -> bool {
        self.dirty
            && self
                .changed
                .is_none_or(|changed| changed.elapsed() >= Duration::from_millis(250))
            && self
                .attempted
                .is_none_or(|attempted| attempted.elapsed() >= Duration::from_secs(1))
    }
}

pub(crate) struct Watch {
    #[cfg(windows)]
    _directory: super::Directory,
    #[cfg(windows)]
    native: transmog_directory_watch::DirectoryWatch,
    changes: Changes,
}
impl Watch {
    pub(crate) fn new(path: &Path, paths: &[String], dirty: bool) -> Result<Self, Error> {
        #[cfg(windows)]
        {
            let directory = super::Directory::open(path)?;
            let native = transmog_directory_watch::DirectoryWatch::open(&directory.root)?;
            let mut changes = Changes {
                dirty,
                ..Changes::default()
            };
            changes.update(paths);
            Ok(Self {
                _directory: directory,
                native,
                changes,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (path, paths, dirty);
            Err(Error::Invalid(
                "Source notifications are unavailable on this platform".into(),
            ))
        }
    }
    pub(crate) fn update(&mut self, paths: &[String]) {
        self.changes.update(paths);
    }
    pub(crate) fn poll(&mut self) -> Result<bool, Error> {
        #[cfg(windows)]
        if let Some(notifications) = self.native.poll()?
            && (notifications.overflowed
                || notifications
                    .names
                    .iter()
                    .any(|name| self.changes.relevant(name)))
        {
            self.changes.mark();
        }
        Ok(self.changes.ready())
    }
    pub(crate) fn acknowledge(&mut self) {
        let _ = self.poll();
        self.changes.dirty = false;
        self.changes.changed = None;
        self.changes.attempted = Some(Instant::now());
    }
}
