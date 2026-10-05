use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::{AppError, ErrorCategory};

const CURRENT_SCHEMA: u32 = 2;
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
    /// Default bounded session page size.
    pub session_page_size: usize,
    /// Whether a proxy start should request current-user host integration.
    pub configure_system_proxy: bool,
}

impl Default for ProductPreferences {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            session_page_size: 100,
            configure_system_proxy: false,
        }
    }
}

/// Explicit persisted privacy choices.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacySettings {
    /// Default to retaining bounded body samples in new captures.
    pub retain_body_samples: bool,
    /// Permit recent artifact paths to be retained locally.
    pub remember_recent_artifacts: bool,
    /// Permit paths in manually-created support bundles.
    pub include_paths_in_support_bundles: bool,
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
}

impl Default for ProductState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            preferences: ProductPreferences::default(),
            privacy: PrivacySettings::default(),
            window: WindowState::default(),
            recent_artifacts: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductStateV1 {
    schema_version: u32,
    window_width: u32,
    window_height: u32,
    remember_recent_artifacts: bool,
    #[serde(default)]
    recent_artifacts: Vec<PathBuf>,
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
        let state = validate(state)?;
        let mut inner = self.inner.lock().unwrap();
        if let Some(prefix) = &self.prefix {
            write_generation(prefix, inner.next_generation, &state)?;
            inner.next_generation = inner.next_generation.saturating_add(1);
            prune_generations(prefix);
        }
        inner.state = state.clone();
        Ok(state)
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

fn validate(mut state: ProductState) -> Result<ProductState, AppError> {
    if state.schema_version != CURRENT_SCHEMA {
        return Err(invalid("unsupported product-state schema"));
    }
    if !(640..=16_384).contains(&state.window.width)
        || !(480..=16_384).contains(&state.window.height)
        || !(10..=200).contains(&state.preferences.session_page_size)
        || state.recent_artifacts.len() > MAX_RECENT_ARTIFACTS
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
    let state = match schema {
        1 => {
            let old: ProductStateV1 = serde_json::from_value(value).map_err(|_| ())?;
            if old.schema_version != 1 {
                return Err(());
            }
            ProductState {
                window: WindowState {
                    width: old.window_width,
                    height: old.window_height,
                    ..WindowState::default()
                },
                privacy: PrivacySettings {
                    remember_recent_artifacts: old.remember_recent_artifacts,
                    ..PrivacySettings::default()
                },
                recent_artifacts: old
                    .recent_artifacts
                    .into_iter()
                    .map(|path| RecentArtifact {
                        path,
                        kind: ArtifactKind::NativeCapture,
                    })
                    .collect(),
                ..ProductState::default()
            }
        }
        2 => serde_json::from_value(value).map_err(|_| ())?,
        _ => return Err(()),
    };
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
    fn corrupt_newest_falls_back_and_v1_migrates() {
        let path = prefix("recovery");
        cleanup(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(generation_path(&path, 1), br#"{"schemaVersion":1,"windowWidth":900,"windowHeight":700,"rememberRecentArtifacts":true,"recentArtifacts":["one.tmcap"]}"#).unwrap();
        fs::write(generation_path(&path, 2), b"{broken").unwrap();
        let (store, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_some());
        assert_eq!(store.snapshot().window.width, 900);
        assert_eq!(store.snapshot().schema_version, CURRENT_SCHEMA);
        cleanup(&path);
    }

    #[test]
    fn validates_bounds_and_privacy() {
        let store = ProductStateManager::memory(ProductState::default());
        let mut invalid = store.snapshot();
        invalid.window.width = 1;
        assert!(store.save(invalid).is_err());
        store.remember(PathBuf::from("secret.tmcap"), ArtifactKind::NativeCapture);
        assert!(store.snapshot().recent_artifacts.is_empty());
    }
}
