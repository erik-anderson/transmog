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

const CURRENT_SCHEMA: u32 = 10;
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
#[allow(clippy::struct_excessive_bools)] // Independent persisted user choices.
pub struct ProductPreferences {
    /// Independently saved profile definitions and the explicitly active recipe.
    #[serde(default)]
    pub network_profiles: crate::NetworkProfiles,
    /// Explicitly allow desktop proxy connections from other computers.
    #[serde(default)]
    pub allow_remote_clients: bool,
    /// Desired desktop listener port; None retains automatic ephemeral selection.
    #[serde(default)]
    pub proxy_port: Option<u16>,
    /// Compatibility mirror of the active profile's reuse choice; edit through network profiles.
    #[serde(default = "enabled_by_default")]
    pub reuse_server_connections: bool,
    /// Public host authorization only; private keys and transient answers are never persisted.
    #[serde(default)]
    pub client_certificate_policy: transmog_core::client_authentication::ClientCertificatePolicy,
    /// Decrypt HTTPS traffic; false forwards CONNECT contents without inspection.
    #[serde(default = "enabled_by_default")]
    pub decrypt_https: bool,
    /// Partition origin connections by caller PID/source IP and downstream H2 session.
    #[serde(default = "enabled_by_default")]
    pub isolate_outbound_connections: bool,
    /// Compatibility mirror of the active profile's allowed protocols; edit through network profiles.
    #[serde(default)]
    pub allowed_outbound_protocols: transmog_core::connection_policy::AllowedHttpProtocols,
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
            network_profiles: crate::NetworkProfiles::default(),
            client_certificate_policy:
                transmog_core::client_authentication::ClientCertificatePolicy::default(),
            allow_remote_clients: false,
            proxy_port: None,
            reuse_server_connections: true,
            decrypt_https: true,
            isolate_outbound_connections: true,
            allowed_outbound_protocols:
                transmog_core::connection_policy::AllowedHttpProtocols::default(),
            theme: ThemePreference::System,
            session_page_size: 100,
            configure_system_proxy: true,
        }
    }
}

const fn enabled_by_default() -> bool {
    true
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
    /// Preserve the authored Custom choice even when its finite value equals the preset.
    #[serde(default)]
    pub custom_request_body_limit: bool,
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
            custom_request_body_limit: false,
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
        let allowed_outbound_protocols = inner
            .state
            .preferences
            .network_profiles
            .active_profile
            .settings
            .protocols;
        let reuse_server_connections = inner
            .state
            .preferences
            .network_profiles
            .active_profile
            .settings
            .reuse_connections;
        let state = validate(ProductState {
            preferences: ProductPreferences {
                allowed_outbound_protocols,
                reuse_server_connections,
                network_profiles: inner.state.preferences.network_profiles.clone(),
                client_certificate_policy: inner
                    .state
                    .preferences
                    .client_certificate_policy
                    .clone(),
                ..state.preferences
            },
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

    pub(crate) fn save_client_certificate_policy(
        &self,
        policy: transmog_core::client_authentication::ClientCertificatePolicy,
    ) -> Result<transmog_core::client_authentication::ClientCertificatePolicy, AppError> {
        let mut inner = self.inner.lock().unwrap();
        let mut state = inner.state.clone();
        state.preferences.client_certificate_policy = policy;
        let state = validate(state)?;
        self.persist(&mut inner, &state)?;
        Ok(state.preferences.client_certificate_policy)
    }

    pub(crate) fn edit_network_profiles(
        &self,
        expected_revision: u64,
        profiles: Vec<crate::NetworkProfileSnapshot>,
        use_profile: Option<&str>,
    ) -> Result<crate::NetworkProfiles, AppError> {
        self.snapshot().preferences.network_profiles.edited(
            expected_revision,
            profiles,
            use_profile,
        )
    }

    pub(crate) fn save_network_profiles(
        &self,
        expected_revision: u64,
        profiles: crate::NetworkProfiles,
    ) -> Result<crate::NetworkProfiles, AppError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.state.preferences.network_profiles.revision != expected_revision {
            return Err(invalid(
                "Network profiles changed in another window. Reload profiles before saving; your draft is still available.",
            ));
        }
        profiles.validate()?;
        let mut state = inner.state.clone();
        state.preferences.allowed_outbound_protocols = profiles.active_profile.settings.protocols;
        state.preferences.reuse_server_connections =
            profiles.active_profile.settings.reuse_connections;
        state.preferences.network_profiles = profiles;
        self.persist(&mut inner, &state)?;
        Ok(state.preferences.network_profiles)
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
    if state.schema_version == 6 {
        state.preferences.network_profiles = crate::NetworkProfiles::migrate(
            state.preferences.allowed_outbound_protocols,
            state.preferences.reuse_server_connections,
        );
        state.schema_version = CURRENT_SCHEMA;
    }
    if state.schema_version == 7 {
        state.preferences.network_profiles.migrate_alps()?;
        state.schema_version = CURRENT_SCHEMA;
    }
    if state.schema_version == 8 {
        state
            .preferences
            .network_profiles
            .migrate_trust_anchor_ids()?;
        state.schema_version = CURRENT_SCHEMA;
    }
    if state.schema_version == 9 {
        state.preferences.network_profiles.migrate_root_stores()?;
        state.schema_version = CURRENT_SCHEMA;
    }
    if state.schema_version != CURRENT_SCHEMA {
        return Err(invalid("unsupported product-state schema"));
    }
    if state.preferences.proxy_port == Some(0) {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "Proxy port must be between 1 and 65535, or automatic.",
            false,
        ));
    }
    state.preferences.network_profiles.validate()?;
    state.preferences.allowed_outbound_protocols = state
        .preferences
        .network_profiles
        .active_profile
        .settings
        .protocols;
    state.preferences.reuse_server_connections = state
        .preferences
        .network_profiles
        .active_profile
        .settings
        .reuse_connections;
    if state
        .privacy
        .request_body_limit
        .is_some_and(|bytes| !(1_000_000..=1_000_000_000_000).contains(&bytes))
    {
        return Err(invalid(
            "Request body limit must be from 1 MB to 1 TB, or Unlimited.",
        ));
    }
    if !state.preferences.client_certificate_policy.valid() {
        return Err(invalid(
            "Client certificate rules require an exact host, a valid port and a certificate selector for automatic selection",
        ));
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
        .map_or(Some(1), |(generation, _)| generation.checked_add(1));
    let mut corrupt = Vec::new();
    for (_, path) in entries {
        if let Ok(state) = read_state(&path) {
            if corrupt.is_empty() {
                return (state, next.unwrap_or(u64::MAX), None);
            }
            return recover_state(
                prefix,
                state,
                next,
                &corrupt,
                "newest product state was corrupt; recovered a prior valid generation with remote access and client certificate rules disabled",
            );
        }
        corrupt.push(path);
    }
    if corrupt.is_empty() {
        return (ProductState::default(), next.unwrap_or(u64::MAX), None);
    }
    recover_state(
        prefix,
        ProductState::default(),
        next,
        &corrupt,
        "product state was corrupt; safe defaults were loaded",
    )
}

fn recover_state(
    prefix: &Path,
    mut state: ProductState,
    next: Option<u64>,
    corrupt: &[PathBuf],
    warning: &str,
) -> (ProductState, u64, Option<String>) {
    // The missing latest state might have revoked disclosure. Persist the safe
    // recovery before removing evidence of that uncertainty from later loads.
    state.preferences.allow_remote_clients = false;
    state.preferences.client_certificate_policy.default =
        transmog_core::client_authentication::ClientCertificateDefault::NoCertificate;
    for rule in &mut state.preferences.client_certificate_policy.rules {
        rule.enabled = false;
    }
    if let Some(generation) = next
        && write_generation(prefix, generation, &state).is_ok()
    {
        for path in corrupt {
            quarantine(path);
        }
        prune_generations(prefix);
        (
            state,
            generation.saturating_add(1),
            Some(warning.to_owned()),
        )
    } else {
        // Keep the corrupt generations when persistence fails. The next launch
        // must not mistake the older authorization for an ordinary valid load.
        (
            state,
            next.unwrap_or(u64::MAX),
            Some(format!(
                "{warning}; recovery could not be saved, so client certificate rules remain disabled"
            )),
        )
    }
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
    if schema != u64::from(CURRENT_SCHEMA) && !(6..=9).contains(&schema) {
        return Err(());
    }
    if schema >= 7
        && value
            .get("preferences")
            .and_then(|preferences| preferences.get("networkProfiles"))
            .is_none()
    {
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

    #[test]
    fn schema_seven_alps_migration_preserves_custom_profiles_and_activates_the_builtin_revision() {
        let mut previous = ProductState {
            schema_version: 7,
            ..Default::default()
        };
        previous.preferences.network_profiles.active_profile =
            crate::NetworkProfileSnapshot::legacy_chromium(1);
        previous
            .preferences
            .network_profiles
            .active_profile
            .revision = 1;
        previous
            .preferences
            .network_profiles
            .active_profile
            .settings
            .http2_alps = false;
        previous
            .preferences
            .network_profiles
            .active_profile
            .settings
            .trust_anchor_ids = false;
        let mut custom = previous.preferences.network_profiles.active_profile.clone();
        custom.id = "custom-diagnostic".into();
        custom.name = "Diagnostic".into();
        previous
            .preferences
            .network_profiles
            .custom_profiles
            .push(custom.clone());
        let mut value = serde_json::to_value(previous.clone()).unwrap();
        let profiles = &mut value["preferences"]["networkProfiles"];
        profiles["activeProfile"]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("http2Alps");
        profiles["customProfiles"][0]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("http2Alps");
        let migrated = validate(serde_json::from_value(value).unwrap()).unwrap();
        assert_eq!(migrated.schema_version, CURRENT_SCHEMA);
        assert_eq!(
            migrated.preferences.network_profiles.active_profile,
            crate::NetworkProfileSnapshot::default()
        );
        assert_eq!(
            migrated.preferences.network_profiles.custom_profiles,
            [custom.clone()]
        );
        assert_eq!(migrated.preferences.network_profiles.revision, 2);
        previous.preferences.network_profiles.active_profile = custom;
        let migrated = validate(previous).unwrap();
        assert!(
            !migrated
                .preferences
                .network_profiles
                .active_profile
                .settings
                .http2_alps
        );
        assert_eq!(
            migrated.preferences.network_profiles.active_profile,
            migrated.preferences.network_profiles.custom_profiles[0]
        );
    }

    #[test]
    fn schema_eight_migration_keeps_custom_offers_and_updates_the_builtin() {
        let mut state = ProductState {
            schema_version: 8,
            ..Default::default()
        };
        state.preferences.network_profiles.active_profile =
            crate::NetworkProfileSnapshot::legacy_chromium(2);
        state.preferences.network_profiles.active_profile.revision = 2;
        state
            .preferences
            .network_profiles
            .active_profile
            .settings
            .trust_anchor_ids = false;
        let mut custom = state.preferences.network_profiles.active_profile.clone();
        custom.id = "custom-before-tai".into();
        custom.name = "Before TAI".into();
        state
            .preferences
            .network_profiles
            .custom_profiles
            .push(custom.clone());
        let mut json = serde_json::to_value(&state).unwrap();
        json["preferences"]["networkProfiles"]["activeProfile"]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("trustAnchorIds");
        json["preferences"]["networkProfiles"]["customProfiles"][0]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("trustAnchorIds");
        let result = validate(serde_json::from_value(json).unwrap()).unwrap();
        assert_eq!(result.schema_version, CURRENT_SCHEMA);
        assert_eq!(
            result.preferences.network_profiles.active_profile,
            crate::NetworkProfileSnapshot::default()
        );
        assert_eq!(
            result.preferences.network_profiles.custom_profiles,
            [custom.clone()]
        );
        state.preferences.network_profiles.active_profile = custom;
        assert!(
            !validate(state)
                .unwrap()
                .preferences
                .network_profiles
                .active_profile
                .settings
                .trust_anchor_ids
        );
    }

    #[test]
    fn listener_and_pool_preferences_survive_restart_and_reject_zero_port() {
        let path = prefix("listener-pool");
        cleanup(&path);
        let (store, _) = ProductStateManager::load(Some(path.clone()));
        let mut state = store.snapshot();
        state.preferences.allow_remote_clients = true;
        state.preferences.proxy_port = Some(65535);
        state.preferences.reuse_server_connections = false;
        let saved = store.save(state.clone()).unwrap();
        assert_eq!(
            ProductStateManager::load(Some(path.clone())).0.snapshot(),
            saved
        );
        state.preferences.proxy_port = Some(0);
        assert!(store.save(state).is_err());
        assert_eq!(store.snapshot(), saved);
        cleanup(&path);
    }

    #[test]
    fn schema_nine_files_migrate_to_webview2_and_preserve_authored_os_trust() {
        let path = prefix("root-store-migration");
        cleanup(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut old = ProductState {
            schema_version: 9,
            ..Default::default()
        };
        old.preferences.network_profiles.active_profile =
            crate::NetworkProfileSnapshot::legacy_chromium(3);
        let mut custom = old.preferences.network_profiles.active_profile.clone();
        custom.id = "custom-os-roots".into();
        custom.name = "OS roots".into();
        old.preferences
            .network_profiles
            .custom_profiles
            .push(custom.clone());
        let mut json = serde_json::to_value(&old).unwrap();
        json["preferences"]["networkProfiles"]["activeProfile"]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("rootStore");
        json["preferences"]["networkProfiles"]["customProfiles"][0]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("rootStore");
        fs::write(
            generation_path(&path, 1),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
        let (store, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(
            store
                .snapshot()
                .preferences
                .network_profiles
                .active_profile
                .id,
            crate::WEBVIEW2_154
        );
        assert_eq!(
            store
                .snapshot()
                .preferences
                .network_profiles
                .custom_profiles,
            [custom]
        );
        store.save(store.snapshot()).unwrap();
        let (reopened, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(reopened.snapshot(), store.snapshot());
        cleanup(&path);
    }

    #[test]
    fn schema_six_without_profiles_loads_and_saves_a_preserved_custom_recipe() {
        let path = prefix("profile-migration");
        cleanup(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut value = serde_json::to_value(ProductState::default()).unwrap();
        value["schemaVersion"] = serde_json::json!(6);
        value["preferences"]
            .as_object_mut()
            .unwrap()
            .remove("networkProfiles");
        value["preferences"]["allowedOutboundProtocols"]["http2"] = serde_json::json!(false);
        fs::write(
            generation_path(&path, 1),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let (store, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(store.snapshot().schema_version, CURRENT_SCHEMA);
        let migrated = store.snapshot().preferences.network_profiles;
        assert_eq!(migrated.active_profile.name, "Current settings");
        assert!(!migrated.active_profile.settings.protocols.http2);
        let next = store
            .edit_network_profiles(
                migrated.revision,
                migrated.custom_profiles,
                Some(crate::CHROMIUM_154),
            )
            .unwrap();
        store
            .save_network_profiles(migrated.revision, next)
            .unwrap();
        let (loaded, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(
            loaded
                .snapshot()
                .preferences
                .network_profiles
                .active_profile,
            crate::NetworkProfileSnapshot::builtin(crate::CHROMIUM_154).unwrap()
        );
        cleanup(&path);
    }

    #[test]
    fn existing_preferences_gain_safe_network_defaults_without_losing_user_choices() {
        let mut value = serde_json::to_value(ProductState::default()).unwrap();
        let preferences = value["preferences"].as_object_mut().unwrap();
        preferences.remove("isolateOutboundConnections");
        preferences.remove("decryptHttps");
        preferences.remove("allowRemoteClients");
        preferences.remove("proxyPort");
        preferences.remove("reuseServerConnections");
        preferences.remove("allowedOutboundProtocols");
        preferences.remove("clientCertificatePolicy");
        preferences.insert("configureSystemProxy".into(), serde_json::json!(false));
        let restored: ProductState = serde_json::from_value(value).unwrap();
        assert!(restored.preferences.isolate_outbound_connections);
        assert!(restored.preferences.decrypt_https);
        assert!(!restored.preferences.allow_remote_clients);
        assert_eq!(restored.preferences.proxy_port, None);
        assert!(restored.preferences.reuse_server_connections);
        assert_eq!(
            restored.preferences.allowed_outbound_protocols,
            transmog_core::connection_policy::AllowedHttpProtocols::default()
        );
        assert!(!restored.preferences.configure_system_proxy);
        assert_eq!(
            restored.preferences.client_certificate_policy,
            transmog_core::client_authentication::ClientCertificatePolicy::default()
        );
    }

    #[test]
    fn certificate_policy_survives_restart_and_stale_settings_and_layout_saves() {
        let path = prefix("client-certificates");
        cleanup(&path);
        let (store, _) = ProductStateManager::load(Some(path.clone()));
        let mut stale = store.snapshot();
        let policy =
            transmog_core::client_authentication::ClientCertificatePolicy {
                default: transmog_core::client_authentication::ClientCertificateDefault::Prompt,
                rules: vec![transmog_core::client_authentication::ClientCertificateRule {
                id: "host".into(), enabled: true, host: "localhost".into(), port: 443,
                mode: transmog_core::client_authentication::ClientCertificateAction::Automatic {
                    selector: transmog_core::client_authentication::ClientCertificateSelector {
                        identity: Some("public-fingerprint".into()), ..Default::default()
                    },
                },
            }],
            };
        store
            .save_client_certificate_policy(policy.clone())
            .unwrap();
        stale.preferences.theme = ThemePreference::Dark;
        store.save(stale).unwrap();
        let mut workspace = store.snapshot().workspace;
        workspace.sidebar_collapsed = true;
        store.save_workspace(workspace).unwrap();
        let (reloaded, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert!(!reloaded.snapshot().preferences.allow_remote_clients);
        assert_eq!(
            reloaded.snapshot().preferences.client_certificate_policy,
            policy
        );
        assert_eq!(reloaded.snapshot().preferences.theme, ThemePreference::Dark);
        assert!(reloaded.snapshot().workspace.sidebar_collapsed);
        let mut invalid = policy.clone();
        invalid.rules[0].host = "https://invalid.example/path".into();
        assert!(reloaded.save_client_certificate_policy(invalid).is_err());
        assert_eq!(
            reloaded.snapshot().preferences.client_certificate_policy,
            policy
        );
        cleanup(&path);
    }

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
        current.preferences.allow_remote_clients = true;
        current.preferences.client_certificate_policy = transmog_core::client_authentication::ClientCertificatePolicy {
            default: transmog_core::client_authentication::ClientCertificateDefault::Prompt,
            rules: vec![transmog_core::client_authentication::ClientCertificateRule { id: "recovered".into(), enabled: true, host: "localhost".into(), port: 443,
                mode: transmog_core::client_authentication::ClientCertificateAction::Automatic { selector: transmog_core::client_authentication::ClientCertificateSelector { identity: Some("public-certificate".into()), ..Default::default() } }
            }],
        };
        fs::write(
            generation_path(&path, 1),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
        fs::write(generation_path(&path, 2), b"{broken").unwrap();
        let (store, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_some());
        assert_eq!(store.snapshot().window.width, 900);
        assert!(!store.snapshot().preferences.allow_remote_clients);
        assert_eq!(store.snapshot().schema_version, CURRENT_SCHEMA);
        assert_eq!(
            store
                .snapshot()
                .preferences
                .client_certificate_policy
                .default,
            transmog_core::client_authentication::ClientCertificateDefault::NoCertificate
        );
        assert!(!store.snapshot().preferences.client_certificate_policy.rules[0].enabled);
        assert!(
            generation_path(&path, 2)
                .with_extension("json.corrupt")
                .exists()
        );
        let (reloaded, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert!(!reloaded.snapshot().preferences.allow_remote_clients);
        assert_eq!(
            reloaded.snapshot().preferences.client_certificate_policy,
            store.snapshot().preferences.client_certificate_policy
        );
        cleanup(&path);
    }

    #[test]
    fn failed_recovery_persistence_preserves_uncertainty_across_launches() {
        let path = prefix("recovery-write-failure");
        cleanup(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut current = ProductState::default();
        current.preferences.client_certificate_policy.default =
            transmog_core::client_authentication::ClientCertificateDefault::Prompt;
        fs::write(
            generation_path(&path, 1),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
        fs::write(generation_path(&path, 2), b"{broken").unwrap();
        let blocked_temp = path.parent().unwrap().join(format!(
            ".transmog-state-{:020}-{}.tmp",
            3,
            std::process::id()
        ));
        fs::create_dir(&blocked_temp).unwrap();
        for _ in 0..2 {
            let (store, warning) = ProductStateManager::load(Some(path.clone()));
            assert!(warning.unwrap().contains("recovery could not be saved"));
            assert_eq!(
                store
                    .snapshot()
                    .preferences
                    .client_certificate_policy
                    .default,
                transmog_core::client_authentication::ClientCertificateDefault::NoCertificate
            );
            assert!(generation_path(&path, 2).exists());
        }
        fs::remove_dir(blocked_temp).unwrap();
        let (_, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(!warning.unwrap().contains("recovery could not be saved"));
        let (reloaded, warning) = ProductStateManager::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(
            reloaded
                .snapshot()
                .preferences
                .client_certificate_policy
                .default,
            transmog_core::client_authentication::ClientCertificateDefault::NoCertificate
        );
        cleanup(&path);
    }

    #[test]
    fn exhausted_recovery_generation_never_quarantines_the_uncertain_latest_state() {
        let path = prefix("recovery-exhausted");
        cleanup(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut current = ProductState::default();
        current.preferences.client_certificate_policy.default =
            transmog_core::client_authentication::ClientCertificateDefault::Prompt;
        fs::write(
            generation_path(&path, 1),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
        fs::write(generation_path(&path, u64::MAX), b"{broken").unwrap();
        for _ in 0..2 {
            let (store, warning) = ProductStateManager::load(Some(path.clone()));
            assert!(warning.unwrap().contains("recovery could not be saved"));
            assert_eq!(
                store
                    .snapshot()
                    .preferences
                    .client_certificate_policy
                    .default,
                transmog_core::client_authentication::ClientCertificateDefault::NoCertificate
            );
            assert!(generation_path(&path, u64::MAX).exists());
        }
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
    fn custom_body_limit_boundaries_and_choice_survive_reload() {
        let path = prefix("custom-body-limit");
        cleanup(&path);
        let (store, _) = ProductStateManager::load(Some(path.clone()));
        for bytes in [1_000_000, 1_500_000, 25_000_000, 1_000_000_000_000] {
            let mut state = store.snapshot();
            state.privacy.request_body_limit = Some(bytes);
            state.privacy.custom_request_body_limit = true;
            let saved = store.save(state).unwrap();
            assert_eq!(
                ProductStateManager::load(Some(path.clone())).0.snapshot(),
                saved
            );
        }
        for bytes in [0, 999_999, 1_000_000_000_001, u64::MAX] {
            let mut state = store.snapshot();
            state.privacy.request_body_limit = Some(bytes);
            assert!(store.save(state).is_err());
        }
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
#[test]
fn legacy_network_choices_migrate_and_independent_profile_writes_survive_stale_saves() {
    let mut legacy = ProductState {
        schema_version: 6,
        ..ProductState::default()
    };
    legacy.preferences.allowed_outbound_protocols.http3 = false;
    legacy.preferences.reuse_server_connections = false;
    let migrated = validate(legacy).unwrap();
    assert_eq!(migrated.schema_version, CURRENT_SCHEMA);
    assert_eq!(
        migrated.preferences.network_profiles.active_profile.name,
        "Current settings"
    );
    assert!(
        !migrated
            .preferences
            .network_profiles
            .active_profile
            .settings
            .protocols
            .http3
    );
    assert!(
        !migrated
            .preferences
            .network_profiles
            .active_profile
            .settings
            .reuse_connections
    );
    let store = ProductStateManager::memory(migrated);
    let stale = store.snapshot();
    let catalog = &stale.preferences.network_profiles;
    let next = store
        .edit_network_profiles(
            catalog.revision,
            catalog.custom_profiles.clone(),
            Some(crate::CHROMIUM_154),
        )
        .unwrap();
    store.save_network_profiles(catalog.revision, next).unwrap();
    store.save(stale).unwrap();
    assert!(
        store
            .snapshot()
            .preferences
            .allowed_outbound_protocols
            .http3
    );
    assert!(store.snapshot().preferences.reuse_server_connections);
    assert_eq!(
        store
            .snapshot()
            .preferences
            .network_profiles
            .active_profile
            .id,
        crate::CHROMIUM_154
    );
}
