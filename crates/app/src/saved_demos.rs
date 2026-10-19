//! Dedicated immutable demo objects and atomically published catalogs.
use crate::captured_page::durable::{DurableResource, DurableScene, valid_hash};
use crate::{
    AppError, CapturedPage, CapturedPageOptions, CapturedPageReport, CapturedResource,
    ErrorCategory, ExperimentSelection,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DEMOS: usize = 1024;

/// Immutable identity and expected revision used for explicit replacement or deletion.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedDemoRevision {
    /// Stable demo identity.
    pub id: String,
    /// Exact revision the user reviewed.
    pub revision: u64,
}

/// Frozen demo identity displayed by its isolated playback warning and report.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedDemoSource {
    /// Stable saved demo identity.
    pub id: String,
    /// Name at the time this playback was opened.
    pub name: String,
    /// Exact replacement revision.
    pub revision: u64,
    /// Owning response set.
    pub owner_id: String,
}

/// A durable captured page with the exact saved response changes frozen into it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedDemo {
    /// Stable demo identity, retained across explicit replacement.
    pub id: String,
    /// Owning response set.
    pub owner_id: String,
    /// Immutable owner creation identity; recreating an ID cannot adopt old demos.
    pub owner_incarnation: String,
    /// Friendly user-chosen demo name.
    pub name: String,
    /// Monotonic replacement revision.
    pub revision: u64,
    /// Initial creation UTC milliseconds.
    pub created_at: u64,
    /// Most recent explicit save UTC milliseconds.
    pub modified_at: u64,
    /// Exact saved current collection or named version used for the snapshot.
    pub selection: ExperimentSelection,
    /// Chosen complete captured HTML entry.
    pub entry_id: String,
    /// Original page URL.
    pub url: String,
    /// Resource source scope frozen for this demo.
    pub scope: crate::CapturedPageScope,
    /// Retained response variants.
    pub available: usize,
    /// Unavailable or bounded-out captured variants.
    pub skipped: usize,
    /// Total decoded captured and rule bytes.
    pub bytes: u64,
    /// Current storage problem, when the retained snapshot needs repair or deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability_error: Option<String>,
}

/// Explicit saved configuration and page choices for preparing a demo for review.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveDemoInput {
    /// Owning response set.
    pub owner_id: String,
    /// Friendly demo name.
    pub name: String,
    /// Explicitly chosen captured HTML entry; choosing it does not change the set's page.
    pub entry_id: String,
    /// Exact saved selection and resource scope to freeze.
    pub options: CapturedPageOptions,
    /// Present only after choosing to replace this exact existing revision.
    pub replace: Option<SavedDemoRevision>,
}

/// Prepared immutable bytes retained until this token is saved or canceled.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDemoPreparation {
    /// Window flow token; committing never refreezes current rules or linked files.
    pub token: String,
    /// Name being reviewed.
    pub name: String,
    /// Existing exact demo revision being replaced, if requested.
    pub replace: Option<SavedDemoRevision>,
    /// Frozen preparation decisions, including explicit omission reasons.
    pub report: CapturedPageReport,
}

#[derive(Clone)]
pub(crate) struct SavedDemoRegistry(Arc<Registry>);
struct Registry {
    root: PathBuf,
    _temporary: Option<tempfile::TempDir>,
    state: Mutex<State>,
}
struct State {
    catalog: Catalog,
    owners: HashMap<String, String>,
    prepared: HashMap<String, Prepared>,
    storage_error: Option<AppError>,
    storage_floor: Option<u64>,
}
struct Prepared {
    owner: String,
    incarnation: String,
    name: String,
    replace: Option<SavedDemoRevision>,
    page: Arc<CapturedPage>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    schema: u32,
    generation: u64,
    demos: BTreeMap<String, StoredDemo>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogEnvelope {
    sha256: String,
    catalog: Catalog,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredDemo {
    metadata: SavedDemo,
    manifest: String,
}

impl SavedDemoRegistry {
    pub(crate) fn load_available(root: Option<PathBuf>) -> Self {
        let floor = root
            .as_deref()
            .and_then(|root| fs::read_dir(root).ok())
            .and_then(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| catalog_generation(&entry.file_name().to_string_lossy()))
                    .max()
            });
        Self::load(root.clone()).unwrap_or_else(|failure| {
            Self(Arc::new(Registry {
                root: root.unwrap_or_default(),
                _temporary: None,
                state: Mutex::new(State {
                    catalog: Catalog {
                        schema: 1,
                        ..Default::default()
                    },
                    owners: HashMap::new(),
                    prepared: HashMap::new(),
                    storage_error: Some(failure),
                    storage_floor: floor,
                }),
            }))
        })
    }

    pub(crate) fn load(root: Option<PathBuf>) -> Result<Self, AppError> {
        let temporary = if root.is_none() {
            Some(
                tempfile::Builder::new()
                    .prefix("transmog-saved-demos-")
                    .tempdir()
                    .map_err(|_| unavailable())?,
            )
        } else {
            None
        };
        let root = root.unwrap_or_else(|| {
            temporary
                .as_ref()
                .expect("temporary registry")
                .path()
                .to_owned()
        });
        fs::create_dir_all(&root).map_err(|_| unavailable())?;
        let mut latest = None;
        for entry in fs::read_dir(&root).map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            if entry.file_type().map_err(|_| unavailable())?.is_file()
                && let Some(generation) = catalog_generation(&entry.file_name().to_string_lossy())
                && latest
                    .as_ref()
                    .is_none_or(|(current, _)| generation > *current)
            {
                latest = Some((generation, entry.path()));
            }
        }
        let catalog = match latest {
            Some((generation, path)) => {
                let envelope: CatalogEnvelope =
                    serde_json::from_slice(&read_document(&path)?).map_err(|_| corrupt())?;
                if hash(&serde_json::to_vec(&envelope.catalog).map_err(|_| corrupt())?)
                    != envelope.sha256
                {
                    return Err(corrupt());
                }
                let catalog = envelope.catalog;
                if catalog.schema != 1
                    || catalog.generation != generation
                    || catalog.demos.len() > MAX_DEMOS
                    || catalog.demos.iter().any(|(id, item)| {
                        id != &item.metadata.id
                            || !valid_id(id)
                            || !valid_hash(&item.manifest)
                            || item.metadata.revision == 0
                    })
                {
                    return Err(corrupt());
                }
                catalog
            }
            None => Catalog {
                schema: 1,
                ..Default::default()
            },
        };
        Ok(Self(Arc::new(Registry {
            root,
            _temporary: temporary,
            state: Mutex::new(State {
                catalog,
                owners: HashMap::new(),
                prepared: HashMap::new(),
                storage_error: None,
                storage_floor: None,
            }),
        })))
    }

    pub(crate) fn reconcile_owners(
        &self,
        owners: &HashMap<String, String>,
    ) -> Result<(), AppError> {
        let mut state = self.lock();
        state.owners.clone_from(owners);
        self.recover_storage(&mut state)?;
        let mut demos = state.catalog.demos.clone();
        demos.retain(|_, item| {
            owners.get(&item.metadata.owner_id) == Some(&item.metadata.owner_incarnation)
        });
        if demos.len() != state.catalog.demos.len() {
            self.publish(&mut state, demos)?;
        }
        state.owners.clone_from(owners);
        state
            .prepared
            .retain(|_, item| owners.get(&item.owner) == Some(&item.incarnation));
        self.collect(&state)
    }

    pub(crate) fn admit_owner(&self, owner: &str, incarnation: &str) -> Result<(), AppError> {
        let mut state = self.lock();
        if state
            .owners
            .get(owner)
            .is_some_and(|old| old != incarnation)
        {
            return Err(conflict(
                "This response set was replaced; reopen Saved demos",
            ));
        }
        state.owners.insert(owner.into(), incarnation.into());
        self.recover_storage(&mut state)?;
        // A previous deletion may have committed while registry cleanup failed.
        let mut demos = state.catalog.demos.clone();
        demos.retain(|_, item| {
            item.metadata.owner_id != owner || item.metadata.owner_incarnation == incarnation
        });
        if demos.len() != state.catalog.demos.len() {
            self.publish(&mut state, demos)?;
        }
        state.owners.insert(owner.into(), incarnation.into());
        Ok(())
    }

    pub(crate) fn invalidate_owner(&self, owner: &str) {
        let mut state = self.lock();
        state.owners.remove(owner);
        state.prepared.retain(|_, item| item.owner != owner);
        if state.storage_error.is_some() {
            return;
        }
        let mut demos = state.catalog.demos.clone();
        demos.retain(|_, item| item.metadata.owner_id != owner);
        // The response set deletion already committed. Inaccessibility is immediate;
        // physical cleanup may be retried by reconciliation on the next startup.
        if self.publish(&mut state, demos).is_ok() {
            let _ = self.collect(&state);
        }
    }

    pub(crate) fn list(&self, owner: &str, incarnation: &str) -> Vec<SavedDemo> {
        let state = self.lock();
        state
            .catalog
            .demos
            .values()
            .filter(|item| {
                item.metadata.owner_id == owner && item.metadata.owner_incarnation == incarnation
            })
            .map(|item| {
                let mut metadata = item.metadata.clone();
                metadata.availability_error = self.read_scene(&item.manifest).and_then(|scene| {
                    for (digest, bytes) in scene.objects() {
                        if fs::metadata(self.0.root.join(format!("body-{digest}.bin"))).map_or(true, |file| !file.is_file() || file.len() != bytes) { return Err(corrupt()); }
                    }
                    Ok(())
                }).err().map(|_| "This demo's stored bytes or metadata are unavailable or damaged. Restore the original files and refresh, replace this demo, or delete it.".into());
                metadata
            })
            .collect()
    }

    pub(crate) fn stage(
        &self,
        owner: &str,
        incarnation: &str,
        name: &str,
        replace: Option<SavedDemoRevision>,
        page: CapturedPage,
    ) -> Result<SavedDemoPreparation, AppError> {
        let name = validate_name(name)?;
        let mut state = self.lock();
        require_owner(&state, owner, incarnation)?;
        check_replace(&state, owner, incarnation, replace.as_ref())?;
        if state.prepared.len() >= 4 {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Finish or cancel another prepared demo before saving a new one",
                false,
            ));
        }
        let report = page.diagnostics().snapshot();
        if report.variant.as_ref().is_none_or(|selection| {
            selection.experiment_id != owner || selection.incarnation != incarnation
        }) {
            return Err(conflict(
                "Choose an exact saved response set revision for this demo",
            ));
        }
        let token = random_id()?;
        state.prepared.insert(
            token.clone(),
            Prepared {
                owner: owner.into(),
                incarnation: incarnation.into(),
                name: name.clone(),
                replace: replace.clone(),
                page: Arc::new(page),
            },
        );
        Ok(SavedDemoPreparation {
            token,
            name,
            replace,
            report,
        })
    }

    pub(crate) fn cancel(&self, token: &str) {
        self.lock().prepared.remove(token);
    }

    pub(crate) fn prepared_owner(&self, token: &str) -> Result<(String, String), AppError> {
        self.lock()
            .prepared
            .get(token)
            .map(|prepared| (prepared.owner.clone(), prepared.incarnation.clone()))
            .ok_or_else(|| conflict("Prepared demo expired or was canceled; prepare it again"))
    }

    pub(crate) fn commit(
        &self,
        token: &str,
        reviewed_missing: bool,
        canceled: &AtomicBool,
    ) -> Result<SavedDemo, AppError> {
        let mut state = self.lock();
        let prepared = state
            .prepared
            .get(token)
            .ok_or_else(|| conflict("Prepared demo expired or was canceled; prepare it again"))?;
        require_owner(&state, &prepared.owner, &prepared.incarnation)?;
        check_replace(
            &state,
            &prepared.owner,
            &prepared.incarnation,
            prepared.replace.as_ref(),
        )?;
        let report = prepared.page.diagnostics().snapshot();
        if report.skipped > 0 && !reviewed_missing {
            return Err(conflict(
                "Review the unavailable resources before saving this demo",
            ));
        }
        if prepared.replace.is_none() && state.catalog.demos.len() >= MAX_DEMOS {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Saved demo storage has reached its 1,024-demo limit",
                false,
            ));
        }
        check_canceled(canceled)?;
        let scene = DurableScene::freeze(&prepared.page, &mut |resource| {
            self.store_resource(resource, canceled)
        })?;
        let manifest = serde_json::to_vec(&scene).map_err(|_| unavailable())?;
        if manifest.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Saved demo metadata exceeds its 16 MiB limit",
                false,
            ));
        }
        let manifest_hash = hash(&manifest);
        store_immutable(
            &self.0.root.join(format!("scene-{manifest_hash}.json")),
            &manifest,
        )?;
        check_canceled(canceled)?;
        let previous = prepared
            .replace
            .as_ref()
            .and_then(|r| state.catalog.demos.get(&r.id));
        let now = now();
        let metadata = SavedDemo {
            id: previous.map_or_else(random_id, |old| Ok(old.metadata.id.clone()))?,
            owner_id: prepared.owner.clone(),
            owner_incarnation: prepared.incarnation.clone(),
            name: prepared.name.clone(),
            revision: previous.map_or(Ok(1), |old| {
                old.metadata.revision.checked_add(1).ok_or_else(corrupt)
            })?,
            created_at: previous.map_or(now, |old| old.metadata.created_at),
            modified_at: now,
            selection: report.variant.clone().ok_or_else(corrupt)?,
            entry_id: report.selected_entry_id,
            url: report.url,
            scope: report.scope,
            available: report.available,
            skipped: report.skipped,
            bytes: report.bytes,
            availability_error: None,
        };
        let mut demos = state.catalog.demos.clone();
        demos.insert(
            metadata.id.clone(),
            StoredDemo {
                metadata: metadata.clone(),
                manifest: manifest_hash,
            },
        );
        self.publish(&mut state, demos)?;
        state.prepared.remove(token);
        let _ = self.collect(&state);
        Ok(metadata)
    }

    pub(crate) fn delete(
        &self,
        owner: &str,
        incarnation: &str,
        expected: &SavedDemoRevision,
    ) -> Result<(), AppError> {
        let mut state = self.lock();
        require_owner(&state, owner, incarnation)?;
        check_replace(&state, owner, incarnation, Some(expected))?;
        let mut demos = state.catalog.demos.clone();
        demos.remove(&expected.id);
        self.publish(&mut state, demos)?;
        let _ = self.collect(&state);
        Ok(())
    }

    pub(crate) fn open(
        &self,
        owner: &str,
        incarnation: &str,
        expected: &SavedDemoRevision,
        canceled: &AtomicBool,
    ) -> Result<CapturedPage, AppError> {
        let state = self.lock();
        require_owner(&state, owner, incarnation)?;
        check_replace(&state, owner, incarnation, Some(expected))?;
        let stored = state.catalog.demos.get(&expected.id).ok_or_else(corrupt)?;
        let scene = self.read_scene(&stored.manifest)?;
        let root = Arc::new(
            tempfile::Builder::new()
                .prefix("transmog-saved-demo-playback-")
                .tempdir()
                .map_err(|_| unavailable())?,
        );
        let mut page = scene.hydrate(root.clone(), &mut |resource| {
            check_canceled(canceled)?;
            let target = root.path().join(format!("{}.body", resource.sha256));
            if !target.exists() {
                let mut input =
                    File::open(self.0.root.join(format!("body-{}.bin", resource.sha256)))
                        .map_err(|_| corrupt())?;
                let mut output = File::create(&target).map_err(|_| unavailable())?;
                let (bytes, digest) = copy_hashed(&mut input, &mut output, canceled)?;
                if bytes != resource.bytes || digest != resource.sha256 {
                    return Err(corrupt());
                }
            }
            Ok(CapturedResource {
                sha256: resource.sha256.clone(),
                status: resource.status,
                reason: resource.reason.clone(),
                headers: resource.headers.clone(),
                body_path: target,
            })
        })?;
        let mut report = page.diagnostics().snapshot();
        report.saved_demo = Some(SavedDemoSource {
            id: stored.metadata.id.clone(),
            name: stored.metadata.name.clone(),
            revision: stored.metadata.revision,
            owner_id: stored.metadata.owner_id.clone(),
        });
        page.diagnostics = crate::CapturedPageDiagnostics::new(report);
        Ok(page)
    }

    fn store_resource(
        &self,
        resource: &CapturedResource,
        canceled: &AtomicBool,
    ) -> Result<DurableResource, AppError> {
        check_canceled(canceled)?;
        if !valid_hash(&resource.sha256) {
            return Err(corrupt());
        }
        let mut input = File::open(&resource.body_path).map_err(|_| unavailable())?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".demo-tmp-")
            .tempfile_in(&self.0.root)
            .map_err(|_| unavailable())?;
        let (bytes, digest) = copy_hashed(&mut input, &mut temporary, canceled)?;
        if digest != resource.sha256 {
            return Err(corrupt());
        }
        temporary.as_file().sync_all().map_err(|_| unavailable())?;
        let destination = self.0.root.join(format!("body-{digest}.bin"));
        if destination.exists() {
            let (stored_bytes, stored_hash) = copy_hashed(
                &mut File::open(&destination).map_err(|_| corrupt())?,
                &mut std::io::sink(),
                canceled,
            )?;
            if stored_bytes != bytes || stored_hash != digest {
                return Err(corrupt());
            }
        } else {
            temporary
                .persist_noclobber(destination)
                .map_err(|_| unavailable())?;
        }
        Ok(DurableResource {
            sha256: digest,
            bytes,
            status: resource.status,
            reason: resource.reason.clone(),
            headers: resource.headers.clone(),
        })
    }

    fn publish(
        &self,
        state: &mut State,
        demos: BTreeMap<String, StoredDemo>,
    ) -> Result<(), AppError> {
        require_storage(state)?;
        let catalog = Catalog {
            schema: 1,
            generation: state
                .catalog
                .generation
                .checked_add(1)
                .ok_or_else(corrupt)?,
            demos,
        };
        let catalog_bytes = serde_json::to_vec(&catalog).map_err(|_| unavailable())?;
        let catalog = CatalogEnvelope {
            sha256: hash(&catalog_bytes),
            catalog,
        };
        let bytes = serde_json::to_vec(&catalog).map_err(|_| unavailable())?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Saved demo catalog is too large",
                false,
            ));
        }
        store_immutable(
            &self
                .0
                .root
                .join(format!("catalog-{:020}.json", catalog.catalog.generation)),
            &bytes,
        )?;
        state.catalog = catalog.catalog;
        Ok(())
    }

    fn read_scene(&self, digest: &str) -> Result<DurableScene, AppError> {
        if !valid_hash(digest) {
            return Err(corrupt());
        }
        let bytes = read_document(&self.0.root.join(format!("scene-{digest}.json")))?;
        if hash(&bytes) != digest {
            return Err(corrupt());
        }
        let scene: DurableScene = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
        scene.validate()?;
        Ok(scene)
    }

    fn collect(&self, state: &State) -> Result<(), AppError> {
        require_storage(state)?;
        let scenes = state
            .catalog
            .demos
            .values()
            .map(|item| item.manifest.clone())
            .collect::<HashSet<_>>();
        let mut bodies = HashSet::new();
        // Read every live manifest before deleting anything: corruption must never
        // make a referenced body appear unreferenced.
        for digest in &scenes {
            bodies.extend(self.read_scene(digest)?.objects().into_keys());
        }
        for entry in fs::read_dir(&self.0.root).map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            if !entry.file_type().map_err(|_| unavailable())?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let remove = if let Some(generation) = catalog_generation(&name) {
                generation != state.catalog.generation
            } else if let Some(digest) = object_digest(&name, "scene-", ".json") {
                !scenes.contains(digest)
            } else if let Some(digest) = object_digest(&name, "body-", ".bin") {
                !bodies.contains(digest)
            } else {
                name.starts_with(".demo-tmp-")
            };
            if remove {
                fs::remove_file(entry.path()).map_err(|_| unavailable())?;
            }
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn recover_storage(&self, state: &mut State) -> Result<(), AppError> {
        if state.storage_error.is_none() {
            return Ok(());
        }
        if self.0.root.as_os_str().is_empty() {
            return require_storage(state);
        }
        let recovered = match Self::load(Some(self.0.root.clone())) {
            Ok(registry) => registry,
            Err(failure) => {
                state.storage_error = Some(failure);
                return require_storage(state);
            }
        };
        let catalog = recovered.lock().catalog.clone();
        if catalog.generation == 0
            || state
                .storage_floor
                .is_some_and(|floor| catalog.generation < floor)
        {
            return require_storage(state);
        }
        state.catalog = catalog;
        state.storage_error = None;
        state.storage_floor = None;
        let mut demos = state.catalog.demos.clone();
        demos.retain(|_, item| {
            state.owners.get(&item.metadata.owner_id) == Some(&item.metadata.owner_incarnation)
        });
        if demos.len() != state.catalog.demos.len() {
            self.publish(state, demos)?;
        }
        Ok(())
    }
}

fn require_storage(state: &State) -> Result<(), AppError> {
    if let Some(failure) = &state.storage_error {
        return Err(AppError::new(
            failure.category,
            format!(
                "{} Restore the saved demo catalog or storage access, then choose Refresh. Existing demo data has been preserved.",
                failure.message
            ),
            failure.retryable,
        ));
    }
    Ok(())
}
fn require_owner(state: &State, owner: &str, incarnation: &str) -> Result<(), AppError> {
    require_storage(state)?;
    if state
        .owners
        .get(owner)
        .is_none_or(|current| current != incarnation)
    {
        return Err(conflict(
            "This response set is unavailable; reopen Response sets",
        ));
    }
    Ok(())
}
fn check_replace(
    state: &State,
    owner: &str,
    incarnation: &str,
    expected: Option<&SavedDemoRevision>,
) -> Result<(), AppError> {
    if let Some(expected) = expected
        && state.catalog.demos.get(&expected.id).is_none_or(|item| {
            item.metadata.owner_id != owner
                || item.metadata.owner_incarnation != incarnation
                || item.metadata.revision != expected.revision
        })
    {
        return Err(conflict(
            "This saved demo changed or was deleted; reopen Saved demos",
        ));
    }
    Ok(())
}
fn validate_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 160 || name.chars().any(char::is_control) {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "Enter a demo name of 1–160 characters",
            false,
        ));
    }
    Ok(name.into())
}
fn check_canceled(canceled: &AtomicBool) -> Result<(), AppError> {
    if canceled.load(Ordering::Acquire) {
        Err(AppError::new(
            ErrorCategory::Canceled,
            "Saving or opening the demo was canceled",
            false,
        ))
    } else {
        Ok(())
    }
}
fn copy_hashed(
    input: &mut impl Read,
    output: &mut impl Write,
    canceled: &AtomicBool,
) -> Result<(u64, String), AppError> {
    let mut total = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        check_canceled(canceled)?;
        let size = input.read(&mut buffer).map_err(|_| unavailable())?;
        if size == 0 {
            break;
        }
        total += size as u64;
        if total > 32 * 1024 * 1024 {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Saved demo response exceeds the 32 MiB limit",
                false,
            ));
        }
        output
            .write_all(&buffer[..size])
            .map_err(|_| unavailable())?;
        hasher.update(&buffer[..size]);
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}
fn store_immutable(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if path.exists() {
        if read_document(path)? == bytes {
            return Ok(());
        }
        return Err(corrupt());
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".demo-tmp-")
        .tempfile_in(path.parent().ok_or_else(unavailable)?)
        .map_err(|_| unavailable())?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| unavailable())?;
    temporary
        .persist_noclobber(path)
        .map_err(|_| unavailable())?;
    Ok(())
}
fn read_document(path: &Path) -> Result<Vec<u8>, AppError> {
    let mut file = File::open(path).map_err(|_| corrupt())?;
    if file.metadata().map_err(|_| corrupt())?.len() > MAX_DOCUMENT_BYTES {
        return Err(corrupt());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| corrupt())?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(corrupt());
    }
    Ok(bytes)
}
fn catalog_generation(name: &str) -> Option<u64> {
    let value = name.strip_prefix("catalog-")?.strip_suffix(".json")?;
    (value.len() == 20 && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}
fn object_digest<'a>(name: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let value = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    valid_hash(value).then_some(value)
}
fn valid_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn random_id() -> Result<String, AppError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
    Ok(format!("{:032x}", u128::from_le_bytes(bytes)))
}
fn now() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
fn unavailable() -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        "Saved demo storage is unavailable; retry after checking available disk space",
        true,
    )
}
fn corrupt() -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        "Saved demo bytes or metadata failed integrity verification",
        false,
    )
}
fn conflict(message: &str) -> AppError {
    AppError::new(ErrorCategory::Conflict, message, false)
}

#[cfg(test)]
#[path = "saved_demos_tests.rs"]
mod tests;
