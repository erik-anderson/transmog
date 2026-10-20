//! Source notifications belong only to active instances, never to installed declarations.
use super::{Error, ExtensionEntry, Origin, Registry, directory, invalid};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use transmog_extensions::Manifest;

pub(super) type ActiveWatch = Arc<Mutex<Result<directory::Watch, ()>>>;
/// Read-only development state; contains no filesystem path or captured evidence.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadStatus {
    /// An unpacked development instance supports explicit reload.
    pub available: bool,
    /// Automatic reload was selected in Extensions settings.
    pub enabled: bool,
    /// A relevant source change settled and is ready for a candidate attempt.
    pub dirty: bool,
    /// A bounded static explanation if OS notifications failed; manual reload remains available.
    pub error: Option<String>,
}
pub(super) fn paths(manifest: &Manifest) -> Vec<String> {
    manifest
        .files
        .iter()
        .filter(|file| {
            std::path::Path::new(&file.path)
                .extension()
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("js") || ext.eq_ignore_ascii_case("mjs")
                })
        })
        .map(|file| file.path.clone())
        .collect()
}
pub(super) fn prepare(entry: &ExtensionEntry) -> Option<ActiveWatch> {
    if entry.catalog.origin != Origin::Development || !entry.hot_reload {
        return None;
    }
    let result = entry.directory.as_deref().ok_or(()).and_then(|path| {
        directory::Watch::new(path, &paths(&entry.catalog.manifest), false).map_err(|_| ())
    });
    Some(Arc::new(Mutex::new(result)))
}
pub(super) fn update(watch: Option<&ActiveWatch>, paths: &[String]) {
    if let Some(watch) = watch
        && let Ok(Ok(watch)) = watch.lock().as_deref_mut()
    {
        watch.update(paths);
    }
}
pub(super) fn stop(watch: &ActiveWatch) {
    let previous = std::mem::replace(
        &mut *watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        Err(()),
    );
    drop(previous);
}
struct WatchSpec {
    id: String,
    available: bool,
    enabled: bool,
    root: Option<std::path::PathBuf>,
    files: Vec<String>,
    watch: Option<ActiveWatch>,
}
impl Registry {
    pub(super) fn stop_watches(&self, id: &str) {
        let watches = self
            .inner
            .state
            .lock()
            .map(|mut state| {
                state
                    .active
                    .values_mut()
                    .filter(|active| active.info.extension_id == id)
                    .filter_map(|active| active.watch.take())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for watch in watches {
            stop(&watch);
        }
    }
    fn watch_spec(&self, owner: &str, instance: &str) -> Result<WatchSpec, Error> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?;
        let active = state
            .active
            .get(instance)
            .filter(|active| active.owner == owner)
            .ok_or(Error::AccessDenied)?;
        let entry = state
            .entries
            .iter()
            .find(|entry| {
                entry.catalog.enabled && entry.catalog.manifest.id == active.info.extension_id
            })
            .ok_or(Error::AccessDenied)?;
        let initialize = entry.hot_reload && active.watch.is_none();
        Ok(WatchSpec {
            id: active.info.extension_id.clone(),
            available: active.info.development,
            enabled: entry.hot_reload,
            root: if initialize {
                entry.directory.clone()
            } else {
                None
            },
            files: if initialize {
                active.watch_files.clone()
            } else {
                Vec::new()
            },
            watch: active.watch.clone(),
        })
    }
    fn observe(&self, owner: &str, instance: &str, spec: &WatchSpec) -> Result<ActiveWatch, Error> {
        if let Some(watch) = &spec.watch {
            return Ok(Arc::clone(watch));
        }
        let root = spec.root.as_deref().ok_or(Error::AccessDenied)?;
        // Enabling an already active view rebuilds once because earlier edits may be missed.
        let created = Arc::new(Mutex::new(
            directory::Watch::new(root, &spec.files, true).map_err(|_| ()),
        ));
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?;
        if !state.entries.iter().any(|entry| {
            entry.catalog.enabled
                && entry.hot_reload
                && entry.directory == spec.root
                && entry.catalog.manifest.id == spec.id
        }) {
            return Err(Error::AccessDenied);
        }
        let active = state
            .active
            .get_mut(instance)
            .filter(|active| active.owner == owner)
            .ok_or(Error::AccessDenied)?;
        let watch = Arc::clone(active.watch.get_or_insert_with(|| Arc::clone(&created)));
        drop(state);
        Ok(watch)
    }
    /// Polls OS metadata notifications only for the exact owned active development instance.
    /// # Errors
    /// Rejects foreign, disabled or unloaded instances. Notification failure is returned in status.
    pub fn reload_status(&self, owner: &str, instance: &str) -> Result<ReloadStatus, Error> {
        let spec = self.watch_spec(owner, instance)?;
        let mut status = ReloadStatus {
            available: spec.available,
            enabled: spec.enabled,
            dirty: false,
            error: None,
        };
        if !status.available || !status.enabled {
            return Ok(status);
        }
        let watch = self.observe(owner, instance, &spec)?;
        {
            let mut result = watch
                .lock()
                .map_err(|_| invalid("Source notifications are unavailable"))?;
            if let Ok(watch) = &mut *result {
                match watch.poll() {
                    Ok(dirty) => status.dirty = dirty,
                    Err(_) => {
                        *result = Err(());
                    }
                }
            }
            if result.is_err() {
                status.error =
                    Some("Source changes could not be observed. Reload manually to retry.".into());
            }
        }
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?;
        if state
            .active
            .get(instance)
            .is_none_or(|active| active.owner != owner)
        {
            return Err(Error::AccessDenied);
        }
        if !state.entries.iter().any(|entry| {
            entry.catalog.enabled && entry.hot_reload && entry.catalog.manifest.id == spec.id
        }) {
            status.enabled = false;
            status.dirty = false;
            status.error = None;
        }
        Ok(status)
    }
    /// Acknowledges the current source hint when a replacement attempt starts; failed edits do not loop.
    pub fn acknowledge_reload(&self, owner: &str, instance: &str) {
        let watch = self.inner.state.lock().ok().and_then(|state| {
            state
                .active
                .get(instance)
                .filter(|active| active.owner == owner)
                .and_then(|active| active.watch.clone())
        });
        if let Some(watch) = watch
            && let Ok(Ok(watch)) = watch.lock().as_deref_mut()
        {
            watch.acknowledge();
        }
    }
}
