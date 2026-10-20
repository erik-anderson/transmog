//! Shared metadata-only extension management; workers belong to explicitly activated instances.
mod directory;
use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub use transmog_extension_supervisor::HostConfig;
use transmog_extension_supervisor::Runner;
use transmog_extensions::{
    CatalogEntry, Error, Failure, Origin, Package, Snapshot, Visualizer, sha256, valid_id,
    write_package,
};

const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;
const MAX_INSTALLED: usize = 128;

/// A metadata-only installed entry for the Extensions settings experience.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionEntry {
    /// Validated declaration summary and enabled state.
    pub catalog: CatalogEntry,
    /// Explicitly registered developer root, absent for installed packages.
    pub directory: Option<PathBuf>,
    /// Source reload is permitted only while a contribution is active.
    pub hot_reload: bool,
}
/// Current management state; listing it does not inspect sources or create workers.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionStatus {
    /// All installed declaration summaries.
    pub entries: Vec<ExtensionEntry>,
    /// Safe storage-recovery explanation; capture remains available.
    pub warning: Option<String>,
}
/// One optional static table column; no executable code or captured values.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationChoice {
    /// Package-qualified stable identity.
    pub key: String,
    /// Inert heading.
    pub name: String,
    /// Package name for disambiguating headings in View settings.
    pub extension_name: String,
}
/// Exact active contribution identity, separate from its package and UI owner.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    /// Host-generated instance ID.
    pub id: String,
    /// Owning extension.
    pub extension_id: String,
    /// Independent contribution within that extension.
    pub visualizer_id: String,
    /// Exact code/declaration revision executing.
    pub digest: String,
    /// Exact executing declaration, including development changes since discovery.
    pub declaration: Visualizer,
}
/// Host-owned retained data and subscriptions tied to an active instance.
pub trait InstanceResources: Any + Send + Sync {
    /// Immediately cancels feeds and releases retained data; repeated calls are harmless.
    fn stop(&self);
}
struct Active {
    info: Instance,
    owner: String,
    runner: Arc<Runner>,
    resources: Option<Arc<dyn InstanceResources>>,
}
impl Active {
    fn stop(self) {
        drop(self);
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        if let Some(resources) = &self.resources {
            resources.stop();
        }
        self.runner.stop();
    }
}
#[derive(Default)]
struct State {
    entries: Vec<ExtensionEntry>,
    active: BTreeMap<String, Active>,
    warning: Option<String>,
}
struct Inner {
    root: Option<PathBuf>,
    host: Option<HostConfig>,
    mutation: Mutex<()>,
    state: Mutex<State>,
}
/// Cloneable registry shared by all windows; enable state and disable/uninstall are global.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                root: None,
                host: None,
                mutation: Mutex::new(()),
                state: Mutex::new(State::default()),
            }),
        }
    }
}
impl Registry {
    /// Loads only validated cached metadata; corrupt state does not prevent app startup.
    pub fn load(root: PathBuf, host: Option<HostConfig>) -> Self {
        let state=match load(&root){
            Ok(entries)=>State{entries,..State::default()},
            Err(_)=>State{warning:Some("Extension settings could not be loaded. Restore the extension catalog before changing installations.".into()),..State::default()},
        };
        Self {
            inner: Arc::new(Inner {
                root: Some(root),
                host,
                mutation: Mutex::new(()),
                state: Mutex::new(state),
            }),
        }
    }
    /// Returns declaration summaries without reading code, developer folders or traffic.
    pub fn status(&self) -> ExtensionStatus {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ExtensionStatus {
            entries: state.entries.clone(),
            warning: state.warning.clone(),
        }
    }
    /// Returns enabled contributions for native discovery, without copying package inventories.
    /// No sources, developer directories or traffic are read.
    pub fn enabled_visualizers(&self) -> Vec<(String, Visualizer)> {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .entries
            .iter()
            .filter(|entry| entry.catalog.enabled)
            .flat_map(|entry| {
                entry
                    .catalog
                    .manifest
                    .visualizers
                    .iter()
                    .cloned()
                    .map(|visualizer| (entry.catalog.manifest.id.clone(), visualizer))
            })
            .collect()
    }
    /// Selects only enabled static columns, without copying modules, inventories or traffic.
    pub fn classification_columns(
        &self,
        keys: &[String],
    ) -> Vec<(String, transmog_extensions::ClassificationColumn)> {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        keys.iter()
            .filter_map(|key| {
                let (package, id) = key.split_once(':')?;
                let entry = state
                    .entries
                    .iter()
                    .find(|entry| entry.catalog.enabled && entry.catalog.manifest.id == package)?;
                let column = entry
                    .catalog
                    .manifest
                    .traffic_columns
                    .iter()
                    .find(|column| column.id == id)?;
                Some((key.clone(), column.clone()))
            })
            .collect()
    }
    /// Lists headings from enabled declarations without copying matching rules or file inventories.
    pub fn classification_choices(&self) -> Vec<ClassificationChoice> {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .entries
            .iter()
            .filter(|entry| entry.catalog.enabled)
            .flat_map(|entry| {
                entry
                    .catalog
                    .manifest
                    .traffic_columns
                    .iter()
                    .map(|column| ClassificationChoice {
                        key: transmog_extensions::classification_key(
                            &entry.catalog.manifest.id,
                            &column.id,
                        ),
                        name: column.name.clone(),
                        extension_name: entry.catalog.manifest.name.clone(),
                    })
            })
            .collect()
    }
    /// Installs a verified local tmext atomically, without executing any module.
    /// # Errors
    /// Rejects invalid packages, conflicting provenance, exhausted capacity and storage failures.
    pub fn install(&self, path: &Path) -> Result<ExtensionEntry, Error> {
        let mut package = Package::open(File::open(path)?)?;
        let snapshot = package.snapshot()?;
        let _mutation = self
            .inner
            .mutation
            .lock()
            .map_err(|_| invalid("Extension management is unavailable"))?;
        let entry = ExtensionEntry {
            catalog: CatalogEntry {
                manifest: snapshot.manifest.clone(),
                digest: snapshot.digest.clone(),
                enabled: true,
                origin: Origin::Installed,
            },
            directory: None,
            hot_reload: false,
        };
        let previous = self
            .status()
            .entries
            .into_iter()
            .find(|e| e.catalog.manifest.id == entry.catalog.manifest.id);
        let entries = self.replacement(entry.clone())?;
        let root = self.root()?;
        std::fs::create_dir_all(root)?;
        let target = package_path(root, &snapshot.manifest.id, &snapshot.digest);
        // Stage a verified immutable revision before publishing its metadata. Reinstalling
        // the same revision can repair a replaced/corrupt archive without changing identity.
        let mut staged = tempfile::NamedTempFile::new_in(root)?;
        write_package(
            staged.as_file_mut(),
            &snapshot,
            &std::sync::atomic::AtomicBool::new(false),
        )?;
        staged.as_file_mut().sync_all()?;
        let existed = target.try_exists()?;
        staged.persist(&target).map_err(|e| Error::Io(e.error))?;
        if let Err(error) = self.publish(entries) {
            if !existed {
                let _ = std::fs::remove_file(&target);
            }
            return Err(error);
        }
        self.stop_extension(&entry.catalog.manifest.id);
        if let Some(previous) = previous
            && previous.catalog.digest != entry.catalog.digest
        {
            remove_package(root, &previous.catalog)?;
        }
        Ok(entry)
    }
    /// Registers an explicit local source directory from its manifest only.
    /// # Errors
    /// Rejects unsafe roots, malformed metadata and identity collisions.
    pub fn register_directory(
        &self,
        path: &Path,
        hot_reload: bool,
    ) -> Result<ExtensionEntry, Error> {
        let directory = directory::Directory::open(path)?;
        let manifest = directory.manifest()?;
        let entry = ExtensionEntry {
            catalog: CatalogEntry {
                digest: sha256(&serde_json::to_vec(&manifest)?),
                manifest,
                enabled: true,
                origin: Origin::Development,
            },
            directory: Some(std::path::absolute(path)?),
            hot_reload,
        };
        let _mutation = self
            .inner
            .mutation
            .lock()
            .map_err(|_| invalid("Extension management is unavailable"))?;
        self.replace(entry.clone())?;
        self.stop_extension(&entry.catalog.manifest.id);
        Ok(entry)
    }
    /// Persists enable state. Enabling never starts a worker or resumes a live feed.
    /// # Errors
    /// Returns missing IDs or storage failures without changing the prior state.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), Error> {
        let _mutation = self
            .inner
            .mutation
            .lock()
            .map_err(|_| invalid("Extension management is unavailable"))?;
        let mut entries = self.status().entries;
        let entry = entries
            .iter_mut()
            .find(|e| e.catalog.manifest.id == id)
            .ok_or_else(|| invalid("Extension is not installed"))?;
        entry.catalog.enabled = enabled;
        self.publish(entries)?;
        if !enabled {
            self.stop_extension(id);
        }
        Ok(())
    }
    /// Changes developer hot reload without inspecting or executing its source files.
    /// # Errors
    /// Rejects non-development entries or persistence failures.
    pub fn set_hot_reload(&self, id: &str, enabled: bool) -> Result<(), Error> {
        let _mutation = self
            .inner
            .mutation
            .lock()
            .map_err(|_| invalid("Extension management is unavailable"))?;
        let mut entries = self.status().entries;
        let entry = entries
            .iter_mut()
            .find(|e| e.catalog.manifest.id == id)
            .ok_or_else(|| invalid("Extension is not installed"))?;
        if entry.catalog.origin != Origin::Development {
            return Err(invalid(
                "Hot reload requires an unpacked development extension",
            ));
        }
        entry.hot_reload = enabled;
        self.publish(entries)
    }
    /// Unregisters and unloads an extension; developer source directories are never deleted.
    /// # Errors
    /// Returns missing IDs, bundled-file ownership or persistence failures.
    pub fn uninstall(&self, id: &str) -> Result<(), Error> {
        let _mutation = self
            .inner
            .mutation
            .lock()
            .map_err(|_| invalid("Extension management is unavailable"))?;
        let mut entries = self.status().entries;
        let entry = entries
            .iter()
            .find(|e| e.catalog.manifest.id == id)
            .ok_or_else(|| invalid("Extension is not installed"))?
            .clone();
        if entry.catalog.origin == Origin::Bundled {
            return Err(invalid(
                "Disable bundled extensions; their files belong to the app installation",
            ));
        }
        entries.retain(|e| e.catalog.manifest.id != id);
        self.publish(entries)?;
        self.stop_extension(id);
        if entry.catalog.origin == Origin::Installed {
            let path = package_path(self.root()?, id, &entry.catalog.digest);
            remove_file(&path)?;
        }
        Ok(())
    }
    /// Loads one exact immutable revision, only for explicit activation or opted-in reload.
    /// # Errors
    /// Rejects disabled/missing entries, replaced packages and unsafe developer files.
    pub fn snapshot(&self, id: &str) -> Result<Snapshot, Error> {
        let entry = self.enabled_entry(id)?;
        let snapshot = match entry.catalog.origin {
            Origin::Development => directory::Directory::open(
                entry
                    .directory
                    .as_deref()
                    .ok_or_else(|| invalid("Developer directory is unavailable"))?,
            )?
            .snapshot()?,
            Origin::Installed | Origin::Bundled => {
                let path = package_path(self.root()?, id, &entry.catalog.digest);
                Package::open(File::open(path)?)?.snapshot()?
            }
        };
        if snapshot.manifest.id != id
            || (entry.catalog.origin != Origin::Development
                && snapshot.digest != entry.catalog.digest)
        {
            return Err(invalid(
                "Extension revision differs from its catalog metadata",
            ));
        }
        Ok(snapshot)
    }
    /// Starts a selected visible instance. No package entry other than this contribution executes.
    /// # Errors
    /// Returns management, source validation, worker initialization or stale activation failures.
    pub fn activate(&self, owner: &str, id: &str, visualizer: &str) -> Result<Instance, Failure> {
        self.activate_cancellable(owner, id, visualizer, || false)
    }
    /// Starts an instance while honoring hidden-pane and global management cancellation.
    /// # Errors
    /// Returns canceled, stale, source validation or sandbox initialization failures.
    pub fn activate_cancellable(
        &self,
        owner: &str,
        id: &str,
        visualizer: &str,
        canceled: impl Fn() -> bool,
    ) -> Result<Instance, Failure> {
        if !valid_id(owner) {
            return Err(Failure::new(
                transmog_extensions::FailureKind::Validation,
                "Invalid visualizer owner",
            ));
        }
        let entry = self.enabled_entry(id).map_err(management_failure)?;
        if canceled() {
            return Err(management_failure(invalid("Activation was canceled")));
        }
        let snapshot = self.snapshot(id).map_err(management_failure)?;
        let digest = snapshot.digest.clone();
        let manifest = snapshot.manifest.clone();
        let declaration = manifest
            .visualizers
            .iter()
            .find(|v| v.id == visualizer)
            .cloned()
            .ok_or_else(|| management_failure(invalid("Unknown visualizer")))?;
        let host = self.inner.host.as_ref().ok_or_else(|| {
            Failure::new(
                transmog_extensions::FailureKind::Unavailable,
                "The installed extension helper is unavailable",
            )
        })?;
        let runner = Runner::start_cancellable(host, snapshot, visualizer, || {
            canceled()
                || self.inner.state.lock().map_or(true, |state| {
                    !state
                        .entries
                        .iter()
                        .any(|e| activation_current(e, &entry, &digest))
                })
        })?;
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(|_| {
            Failure::new(
                transmog_extensions::FailureKind::Unavailable,
                "Visualization identity is unavailable",
            )
        })?;
        let info = Instance {
            id: format!("{:032x}", u128::from_le_bytes(nonce)),
            extension_id: id.into(),
            visualizer_id: visualizer.into(),
            digest,
            declaration,
        };
        // Serialize the final stale check and declaration publication with disable,
        // uninstall and replacement; none can race an instance back into the registry.
        let _mutation = self
            .inner
            .mutation
            .lock()
            .map_err(|_| management_failure(invalid("Extension management is unavailable")))?;
        let mut entries = self.status().entries;
        if canceled() {
            return Err(management_failure(invalid("Activation was canceled")));
        }
        let Some(current) = entries
            .iter_mut()
            .find(|e| activation_current(e, &entry, &info.digest))
        else {
            return Err(Failure::new(
                transmog_extensions::FailureKind::Unavailable,
                "Extension changed during activation",
            ));
        };
        if entry.catalog.origin == Origin::Development {
            current.catalog.manifest = manifest;
            current.catalog.digest.clone_from(&info.digest);
            self.publish(entries).map_err(management_failure)?;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| management_failure(invalid("Extension state is unavailable")))?;
        state.active.insert(
            info.id.clone(),
            Active {
                info: info.clone(),
                owner: owner.into(),
                runner,
                resources: None,
            },
        );
        Ok(info)
    }
    /// Finds an instance only for its owning window/application.
    /// # Errors
    /// Denies guessed instance IDs belonging to another owner.
    pub fn runner(&self, owner: &str, instance: &str) -> Result<Arc<Runner>, Error> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?;
        state
            .active
            .get(instance)
            .filter(|a| a.owner == owner)
            .map(|a| Arc::clone(&a.runner))
            .ok_or(Error::AccessDenied)
    }
    /// Attaches retained-data ownership exactly once, before delivering traffic.
    /// # Errors
    /// Denies stale, foreign or already-attached instances.
    pub fn attach_resources<T: InstanceResources>(
        &self,
        owner: &str,
        instance: &str,
        resources: Arc<T>,
    ) -> Result<(), Error> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?;
        let active = state
            .active
            .get_mut(instance)
            .filter(|a| a.owner == owner && a.resources.is_none())
            .ok_or(Error::AccessDenied)?;
        active.resources = Some(resources);
        Ok(())
    }
    /// Obtains the host's resource type only for the owning application.
    /// # Errors
    /// Denies foreign, stopped or mismatched resource instances.
    pub fn resources<T: InstanceResources>(
        &self,
        owner: &str,
        instance: &str,
    ) -> Result<Arc<T>, Error> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?;
        let resources = state
            .active
            .get(instance)
            .filter(|a| a.owner == owner)
            .and_then(|a| a.resources.clone())
            .ok_or(Error::AccessDenied)?;
        let resources: Arc<dyn Any + Send + Sync> = resources;
        resources.downcast().map_err(|_| Error::AccessDenied)
    }
    /// Unloads one instance without stopping another contribution from the same package.
    pub fn deactivate(&self, owner: &str, instance: &str) {
        let active = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.active.get(instance).is_some_and(|a| a.owner == owner) {
                state.active.remove(instance)
            } else {
                None
            }
        };
        if let Some(active) = active {
            active.stop();
        }
    }
    /// Unloads all instances when their owning application/window closes.
    pub fn deactivate_owner(&self, owner: &str) {
        self.stop_matching(|a| a.owner == owner);
    }
    fn stop_extension(&self, id: &str) {
        self.stop_matching(|a| a.info.extension_id == id);
    }
    fn stop_matching(&self, match_instance: impl Fn(&Active) -> bool) {
        let stopped = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids = state
                .active
                .iter()
                .filter(|(_, a)| match_instance(a))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| state.active.remove(&id))
                .collect::<Vec<_>>()
        };
        for active in stopped {
            active.stop();
        }
    }
    fn enabled_entry(&self, id: &str) -> Result<ExtensionEntry, Error> {
        self.status()
            .entries
            .into_iter()
            .find(|e| e.catalog.manifest.id == id && e.catalog.enabled)
            .ok_or_else(|| invalid("Extension is disabled or not installed"))
    }
    fn root(&self) -> Result<&Path, Error> {
        self.inner
            .root
            .as_deref()
            .ok_or_else(|| invalid("Extension installation storage is not configured"))
    }
    fn replace(&self, entry: ExtensionEntry) -> Result<(), Error> {
        self.publish(self.replacement(entry)?)
    }
    fn replacement(&self, entry: ExtensionEntry) -> Result<Vec<ExtensionEntry>, Error> {
        if self.status().warning.is_some() {
            return Err(invalid(
                "Repair the extension catalog before changing installations",
            ));
        }
        let mut entries = self.status().entries;
        if let Some(index) = entries
            .iter()
            .position(|e| e.catalog.manifest.id == entry.catalog.manifest.id)
        {
            if entries[index].catalog.origin != entry.catalog.origin {
                return Err(invalid(
                    "An extension with this ID and different provenance is already registered",
                ));
            }
            entries[index] = entry;
        } else {
            if entries.len() >= MAX_INSTALLED {
                return Err(Error::Limit);
            }
            entries.push(entry);
        }
        Ok(entries)
    }
    fn publish(&self, entries: Vec<ExtensionEntry>) -> Result<(), Error> {
        if self.status().warning.is_some() {
            return Err(invalid(
                "Repair the extension catalog before changing installations",
            ));
        }
        let root = self.root()?;
        std::fs::create_dir_all(root)?;
        let bytes = serde_json::to_vec(&entries)?;
        if bytes.len() as u64 > MAX_CATALOG_BYTES {
            return Err(Error::Limit);
        }
        let mut staged = tempfile::NamedTempFile::new_in(root)?;
        staged.write_all(&bytes)?;
        staged.as_file_mut().sync_all()?;
        staged
            .persist(root.join("catalog.json"))
            .map_err(|e| Error::Io(e.error))?;
        self.inner
            .state
            .lock()
            .map_err(|_| invalid("Extension state is unavailable"))?
            .entries = entries;
        Ok(())
    }
}
fn load(root: &Path) -> Result<Vec<ExtensionEntry>, Error> {
    let mut file = match File::open(root.join("catalog.json")) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    if file.metadata()?.len() > MAX_CATALOG_BYTES {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(MAX_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(Error::Limit);
    }
    let entries: Vec<ExtensionEntry> = serde_json::from_slice(&bytes)?;
    if entries.len() > MAX_INSTALLED {
        return Err(Error::Limit);
    }
    let mut ids = std::collections::BTreeSet::new();
    for entry in &entries {
        entry
            .catalog
            .manifest
            .validate(entry.catalog.origin == Origin::Development)?;
        if !ids.insert(&entry.catalog.manifest.id)
            || entry.catalog.digest.len() != 64
            || !entry
                .catalog
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || (entry.catalog.origin == Origin::Development) != entry.directory.is_some()
        {
            return Err(invalid("Invalid extension catalog"));
        }
    }
    Ok(entries)
}
fn package_path(root: &Path, id: &str, digest: &str) -> PathBuf {
    root.join(format!("{id}.{digest}.tmext"))
}
fn remove_package(root: &Path, catalog: &CatalogEntry) -> Result<(), Error> {
    remove_file(&package_path(root, &catalog.manifest.id, &catalog.digest))
}
fn remove_file(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn management_failure(_: Error) -> Failure {
    Failure::new(
        transmog_extensions::FailureKind::Validation,
        "Extension metadata or source could not be validated",
    )
}
fn activation_current(current: &ExtensionEntry, expected: &ExtensionEntry, digest: &str) -> bool {
    current.catalog.enabled
        && current.catalog.manifest.id == expected.catalog.manifest.id
        && current.catalog.origin == expected.catalog.origin
        && current.directory == expected.directory
        && (current.catalog.digest == expected.catalog.digest
            || (expected.catalog.origin == Origin::Development && current.catalog.digest == digest))
}
