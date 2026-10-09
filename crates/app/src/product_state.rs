use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::workspace::WorkspacePreferences;
use crate::{AppError, ErrorCategory};

const CURRENT_SCHEMA: u32 = 6;
const MAX_STATE_BYTES: u64 = 256 * 1024;
const MAX_RECENT_ARTIFACTS: usize = 20;
const MAX_GENERATIONS: usize = 3;

/// Persisted window geometry in physical pixels.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    /// Last non-maximized inner width.
    pub width: u32,
    /// Last non-maximized inner height.
    pub height: u32,
    /// Optional outer x coordinate.
    pub x: Option<i32>,
    /// Optional outer y coordinate.
    pub y: Option<i32>,
    /// Whether the window was maximized.
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1180,
            height: 760,
            x: None,
            y: None,
            maximized: false,
        }
    }
}

/// Supported product color preference.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreference {
    /// Follow the operating-system setting.
    #[default]
    System,
    /// Prefer a light palette.
    Light,
    /// Prefer a dark palette.
    Dark,
}

/// Versioned non-sensitive product preferences.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductPreferences {
    /// Color preference.
    pub theme: ThemePreference,
    /// Legacy page preference retained for saved-state compatibility; Traffic is virtualized.
    pub session_page_size: usize,
    /// Whether a proxy start should request current-user host integration.
    pub configure_system_proxy: bool,
}

impl Default for ProductPreferences {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            session_page_size: 100,
            configure_system_proxy: true,
        }
    }
}

/// Explicit persisted privacy choices.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct PrivacySettings {
    /// Aggregate circular buffer limit and memory/disk policy.
    pub buffer_limit: crate::BufferLimit,
    /// Optional maximum live entries; older completed entries drop off first.
    pub max_live_entries: Option<usize>,
    /// Retain request bodies for command generation and replay.
    pub retain_request_bodies: bool,
    /// Retained bytes per request; None is Unlimited within overall storage limits.
    pub request_body_limit: Option<u64>,
    /// Remove credential and cookie values from subsequently captured traffic.
    pub redact_sensitive_headers: bool,
    /// Retain response bodies in the product cache for inspection.
    pub retain_response_bodies: bool,
    /// Default to retaining bounded body samples in new captures.
    pub retain_body_samples: bool,
    /// Permit recent artifact paths to be retained locally.
    pub remember_recent_artifacts: bool,
    /// Permit paths in manually-created support bundles.
    pub include_paths_in_support_bundles: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            buffer_limit: crate::BufferLimit::Automatic,
            max_live_entries: None,
            retain_request_bodies: true,
            request_body_limit: default_request_body_limit(),
            redact_sensitive_headers: false,
            retain_response_bodies: true,
            retain_body_samples: true,
            remember_recent_artifacts: false,
            include_paths_in_support_bundles: false,
        }
    }
}

#[allow(clippy::unnecessary_wraps)] // The default policy shares the optional Unlimited type.
const fn default_request_body_limit() -> Option<u64> {
    Some(transmog_capture::DEFAULT_REQUEST_BODY_CAPTURE_BYTES)
}

/// Type of a recent user-selected artifact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    /// Native streaming capture.
    NativeCapture,
    /// JSON-lines export.
    JsonLines,
    /// SAZ export.
    Saz,
    /// HTTP Archive export.
    Har,
    /// Public interception certificate.
    Certificate,
}

/// One bounded recent artifact reference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentArtifact {
    /// User-selected local path.
    pub path: PathBuf,
    /// Artifact format category.
    pub kind: ArtifactKind,
}

/// Complete versioned product state. Live sessions and control envelopes are
/// intentionally absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductState {
    /// On-disk schema version.
    pub schema_version: u32,
    /// Non-sensitive preferences.
    pub preferences: ProductPreferences,
    /// Explicit privacy choices.
    pub privacy: PrivacySettings,
    /// Last window geometry.
    pub window: WindowState,
    /// Most-recent-first user artifact references.
    pub recent_artifacts: Vec<RecentArtifact>,
    /// Non-sensitive layout and column presentation.
    pub workspace: WorkspacePreferences,
}

impl Default for ProductState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            preferences: ProductPreferences::default(),
            privacy: PrivacySettings::default(),
            window: WindowState::default(),
            recent_artifacts: Vec::new(),
            workspace: WorkspacePreferences::default(),
        }
    }
}

#[derive(Debug)]
struct Inner {
    state: ProductState,
    next_generation: u64,
}

/// Thread-safe product-state owner with optional crash-safe persistence.
#[derive(Clone, Debug)]
pub(crate) struct ProductStateManager {
    prefix: Option<PathBuf>,
    inner: Arc<Mutex<Inner>>,
}

impl ProductStateManager {
    pub(crate) fn load(prefix: Option<PathBuf>) -> (Self, Option<String>) {
        let Some(path) = prefix.as_deref() else {
            return (Self::memory(ProductState::default()), None);
        };
        let (state, next_generation, warning) = load_generations(path);
        (
            Self {
                prefix,
                inner: Arc::new(Mutex::new(Inner {
                    state,
                    next_generation,
                })),
            },
            warning,
        )
    }

    fn memory(state: ProductState) -> Self {
        Self {
            prefix: None,
            inner: Arc::new(Mutex::new(Inner {
                state,
                next_generation: 1,
            })),
        }
    }

    pub(crate) fn snapshot(&self) -> ProductState {
        self.inner.lock().unwrap().state.clone()
    }

    pub(crate) fn save(&self, state: ProductState) -> Result<ProductState, AppError> {
        let mut inner = self.inner.lock().unwrap();
        // Main settings/window writes must not overwrite independently saved layout.
        let state = validate(ProductState {
            workspace: inner.state.workspace.clone(),
            ..state
        })?;
        self.persist(&mut inner, &state)?;
        Ok(state)
    }

    pub(crate) fn save_workspace(
        &self,
        preferences: WorkspacePreferences,
    ) -> Result<WorkspacePreferences, AppError> {
        let mut inner = self.inner.lock().unwrap();
        let state = validate(ProductState {
            workspace: preferences,
            ..inner.state.clone()
        })?;
        self.persist(&mut inner, &state)?;
        Ok(state.workspace)
    }

    fn persist(&self, inner: &mut Inner, state: &ProductState) -> Result<(), AppError> {
        if let Some(prefix) = &self.prefix {
            write_generation(prefix, inner.next_generation, state)?;
            inner.next_generation = inner.next_generation.saturating_add(1);
            prune_generations(prefix);
        }
        inner.state = state.clone();
        Ok(())
    }

    pub(crate) fn remember(&self, path: PathBuf, kind: ArtifactKind) {
        let mut state = self.snapshot();
        if !state.privacy.remember_recent_artifacts {
            return;
        }
        state.recent_artifacts.retain(|entry| entry.path != path);
        state
            .recent_artifacts
            .insert(0, RecentArtifact { path, kind });
        state.recent_artifacts.truncate(MAX_RECENT_ARTIFACTS);
        let _ = self.save(state);
    }
}

pub(crate) fn validate(mut state: ProductState) -> Result<ProductState, AppError> {
    if state.schema_version != CURRENT_SCHEMA {
        return Err(invalid("unsupported product-state schema"));
    }
    if !(640..=16_384).contains(&state.window.width)
        || !(480..=16_384).contains(&state.window.height)
        || !(10..=200).contains(&state.preferences.session_page_size)
        || state.recent_artifacts.len() > MAX_RECENT_ARTIFACTS
        || matches!(
            state.privacy.buffer_limit,
            crate::BufferLimit::Custom { bytes: 0 }
        )
        || state
            .privacy
            .max_live_entries
            .is_some_and(|limit| limit == 0 || limit as u64 > 9_007_199_254_740_991)
        || state.privacy.request_body_limit == Some(0)
        || !state.workspace.is_valid()
    {
        return Err(invalid("product state exceeds a configured bound"));
    }
    for coordinate in [state.window.x, state.window.y].into_iter().flatten() {
        if !(-100_000..=100_000).contains(&coordinate) {
            return Err(invalid("window position exceeds a configured bound"));
        }
    }
    let mut unique = BTreeSet::new();
    state
        .recent_artifacts
        .retain(|entry| unique.insert(entry.path.clone()));
    if !state.privacy.remember_recent_artifacts {
        state.recent_artifacts.clear();
    }
    Ok(state)
}

fn invalid(message: &'static str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}

fn load_generations(prefix: &Path) -> (ProductState, u64, Option<String>) {
    let mut entries = generation_paths(prefix);
    entries.sort_by_key(|(generation, _)| std::cmp::Reverse(*generation));
    let next = entries
        .first()
        .map_or(1, |(generation, _)| generation.saturating_add(1));
    let mut corrupt = false;
    for (_, path) in entries {
        if let Ok(state) = read_state(&path) {
            return (
                state,
                next,
                corrupt.then(|| {
                    "newest product state was corrupt; recovered a prior valid generation"
                        .to_owned()
                }),
            );
        }
        corrupt = true;
        quarantine(&path);
    }
    (
        ProductState::default(),
        next,
        corrupt.then(|| "product state was corrupt; safe defaults were loaded".to_owned()),
    )
}

fn read_state(path: &Path) -> Result<ProductState, ()> {
    let metadata = fs::metadata(path).map_err(|_| ())?;
    if metadata.len() > MAX_STATE_BYTES {
        return Err(());
    }
    let bytes = fs::read(path).map_err(|_| ())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    let schema = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or(())?;
    if schema != u64::from(CURRENT_SCHEMA) {
        return Err(());
    }
    let state: ProductState = serde_json::from_value(value).map_err(|_| ())?;
    validate(state).map_err(|_| ())
}

fn write_generation(prefix: &Path, generation: u64, state: &ProductState) -> Result<(), AppError> {
    let parent = prefix
        .parent()
        .ok_or_else(|| invalid("product-state path requires a parent"))?;
    fs::create_dir_all(parent).map_err(|_| persistence_error())?;
    let bytes = serde_json::to_vec_pretty(state).map_err(|_| persistence_error())?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid("product state exceeds its storage bound"));
    }
    let temp = parent.join(format!(
        ".transmog-state-{generation:020}-{}.tmp",
        std::process::id()
    ));
    let destination = generation_path(prefix, generation);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|_| persistence_error())?;
    let result = file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temp, &destination));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
        return Err(persistence_error());
    }
    Ok(())
}

fn persistence_error() -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        "product state could not be persisted",
        true,
    )
}

fn generation_path(prefix: &Path, generation: u64) -> PathBuf {
    let name = prefix
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("preferences");
    prefix.with_file_name(format!("{name}.{generation:020}.json"))
}

fn generation_paths(prefix: &Path) -> Vec<(u64, PathBuf)> {
    let Some(parent) = prefix.parent() else {
        return Vec::new();
    };
    let Some(name) = prefix.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let marker = format!("{name}.");
    fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let file = path.file_name()?.to_str()?;
            let generation = file
                .strip_prefix(&marker)?
                .strip_suffix(".json")?
                .parse()
                .ok()?;
            Some((generation, path))
        })
        .collect()
}

fn prune_generations(prefix: &Path) {
    let mut entries = generation_paths(prefix);
    entries.sort_by_key(|(generation, _)| std::cmp::Reverse(*generation));
    for (_, path) in entries.into_iter().skip(MAX_GENERATIONS) {
        let _ = fs::remove_file(path);
    }
}

fn quarantine(path: &Path) {
    let mut destination = path.as_os_str().to_os_string();
    destination.push(".corrupt");
    let _ = fs::rename(path, PathBuf::from(destination));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix(label: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "transmog-product-state-{}-{label}",
                std::process::id()
            ))
            .join("preferences")
    }

    fn cleanup(prefix: &Path) {
        if let Some(parent) = prefix.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn generations_are_atomic_bounded_and_reload_newest() {
        let path = prefix("generation");
        cleanup(&path);
        let (store, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        for width in 800..805 {
            let mut state = store.snapshot();
            state.window.width = width;
            store.save(state).unwrap();
        }
        assert!(generation_paths(&path).len() <= MAX_GENERATIONS);
        let (loaded, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(loaded.snapshot().window.width, 804);
        cleanup(&path);
    }

    #[test]
    fn corrupt_newest_falls_back_to_a_current_generation() {
        let path = prefix("recovery");
        cleanup(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut current = ProductState::default();
        current.window.width = 900;
        fs::write(
            generation_path(&path, 1),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
        fs::write(generation_path(&path, 2), b"{broken").unwrap();
        let (store, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_some());
        assert_eq!(store.snapshot().window.width, 900);
        assert_eq!(store.snapshot().schema_version, CURRENT_SCHEMA);
        cleanup(&path);
    }

    #[test]
    fn prior_product_state_schemas_are_rejected_without_migration() {
        let mut value = serde_json::to_value(ProductState::default()).unwrap();
        value["schemaVersion"] = 5.into();
        let path = prefix("prior-schema");
        cleanup(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let generation = generation_path(&path, 1);
        fs::write(&generation, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(read_state(&generation).is_err());
        cleanup(&path);
    }

    #[test]
    fn validates_bounds_and_privacy() {
        let store = ProductStateManager::memory(ProductState::default());
        assert!(store.snapshot().privacy.retain_response_bodies);
        assert!(store.snapshot().privacy.retain_request_bodies);
        assert!(store.snapshot().privacy.retain_body_samples);
        assert!(!store.snapshot().privacy.redact_sensitive_headers);
        assert_eq!(
            store.snapshot().privacy.request_body_limit,
            Some(25_000_000)
        );
        let mut unlimited = store.snapshot();
        unlimited.privacy.request_body_limit = None;
        assert_eq!(
            store.save(unlimited).unwrap().privacy.request_body_limit,
            None
        );
        let mut zero = store.snapshot();
        zero.privacy.request_body_limit = Some(0);
        assert!(store.save(zero).is_err());
        let mut invalid = store.snapshot();
        invalid.window.width = 1;
        assert!(store.save(invalid).is_err());
        store.remember(PathBuf::from("secret.tmcap"), ArtifactKind::NativeCapture);
        assert!(store.snapshot().recent_artifacts.is_empty());
    }

    #[test]
    fn current_state_layout_survives_stale_settings_save() {
        let path = prefix("workspace");
        cleanup(&path);
        let (store, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        let mut stale = store.snapshot();
        assert_eq!(stale.workspace, WorkspacePreferences::default());
        assert!(stale.privacy.retain_request_bodies);
        assert_eq!(stale.privacy.request_body_limit, Some(25_000_000));
        assert!(!stale.privacy.redact_sensitive_headers);
        stale.privacy.redact_sensitive_headers = true;
        let mut layout = stale.workspace.clone();
        layout.sidebar_collapsed = true;
        layout.list_split = 67;
        layout.columns.swap(2, 3);
        layout.columns[2].width = 310;
        store.save_workspace(layout.clone()).unwrap();
        stale.preferences.theme = ThemePreference::Dark;
        stale.window.width = 1100;
        store.save(stale).unwrap();
        let (reloaded, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(reloaded.snapshot().workspace, layout);
        assert_eq!(reloaded.snapshot().preferences.theme, ThemePreference::Dark);
        assert_eq!(reloaded.snapshot().window.width, 1100);
        assert!(reloaded.snapshot().privacy.redact_sensitive_headers);
        cleanup(&path);
    }

    #[test]
    fn invalid_layout_never_replaces_saved_preferences() {
        let store = ProductStateManager::memory(ProductState::default());
        let original = store.snapshot().workspace;
        let mut duplicate = original.clone();
        duplicate.columns[1].id = duplicate.columns[0].id;
        let mut invisible = original.clone();
        for column in &mut invisible.columns {
            column.visible = false;
        }
        let mut narrow = original.clone();
        narrow.columns[0].width = 1;
        let mut extreme_split = original.clone();
        extreme_split.list_split = 100;
        for invalid in [duplicate, invisible, narrow, extreme_split] {
            assert!(store.save_workspace(invalid).is_err());
            assert_eq!(store.snapshot().workspace, original);
        }
    }
}
