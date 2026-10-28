use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::Error;

const MAX_GRANTS: usize = 64;
const MAX_STORE_BYTES: u64 = 1024 * 1024;

/// Explicit capability granted in the desktop's Create access key dialog.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Search metadata and inspect masked request/response evidence.
    Inspect,
    /// Read retained message bodies, which can contain sensitive information.
    ReadBodies,
    /// Start and stop the shared proxy and recording.
    Collect,
    /// Prepare and test disabled modifications.
    PrepareChanges,
    /// Apply temporary changes to matching live traffic.
    Modify,
    /// Send explicitly authored or replayed requests.
    Send,
    /// Read original credential-bearing headers instead of masking them.
    ReadCredentials,
}

/// Explicit traffic boundary. Hostnames are exact, without wildcard matching.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Scope {
    /// All retained and future traffic in the main workspace.
    All,
    /// Existing and future traffic for these exact hosts, on any port.
    Hosts {
        /// Canonical DNS names or IP literals, without schemes or paths.
        hosts: Vec<String>,
    },
    /// Exact retained entries from one application instance; selection changes do not expand it.
    Requests {
        /// Instance generation in which these entries were selected.
        instance_id: String,
        /// Frozen opaque retained exchange identifiers.
        ids: Vec<String>,
    },
}

impl Scope {
    /// Checks a destination against an exact host grant. Request-only grants cannot send new traffic.
    pub fn allows_url(&self, target: &str) -> bool {
        let Ok(url) = url::Url::parse(target) else {
            return false;
        };
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        match self {
            Self::All => true,
            Self::Hosts { hosts } => url
                .host_str()
                .is_some_and(|host| canonical_host(host).is_ok_and(|host| hosts.contains(&host))),
            Self::Requests { .. } => false,
        }
    }

    /// Checks a retained entry in the owning application instance.
    pub fn allows_entry(&self, instance: &str, id: &str, target: &str) -> bool {
        match self {
            Self::Requests { instance_id, ids } => {
                instance_id == instance && ids.iter().any(|item| item == id)
            }
            _ => self.allows_url(target),
        }
    }

    fn validate(&mut self) -> Result<(), Error> {
        match self {
            Self::All => {}
            Self::Hosts { hosts } => {
                if hosts.is_empty() || hosts.len() > 64 {
                    return Err(invalid("Choose between 1 and 64 exact hosts."));
                }
                *hosts = hosts
                    .iter()
                    .map(|host| canonical_host(host))
                    .collect::<Result<_, _>>()?;
                hosts.sort();
                hosts.dedup();
            }
            Self::Requests { instance_id, ids } => {
                if instance_id.len() != 32
                    || !instance_id.bytes().all(|b| b.is_ascii_hexdigit())
                    || ids.is_empty()
                    || ids.len() > 4096
                    || ids.iter().any(|id| id.is_empty() || id.len() > 128)
                {
                    return Err(invalid(
                        "Choose retained requests from the current workspace.",
                    ));
                }
                ids.sort();
                ids.dedup();
            }
        }
        Ok(())
    }
}

fn canonical_host(host: &str) -> Result<String, Error> {
    if host.is_empty()
        || host.len() > 253
        || host.contains(['/', '@', '?', '#', '*'])
        || host.chars().any(char::is_whitespace)
    {
        return Err(invalid(
            "Enter exact hostnames or IP addresses without schemes, ports, paths or wildcards.",
        ));
    }
    url::Host::parse(host)
        .map(|host| host.to_string().trim_end_matches('.').to_ascii_lowercase())
        .map_err(|_| invalid("Enter a valid hostname or IP address."))
}

/// Authored configuration for a new grant or an explicitly edited grant.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantInput {
    /// User-assigned label; never an authentication identifier.
    pub name: String,
    /// Nonempty explicit capabilities.
    pub permissions: BTreeSet<Permission>,
    /// Approved traffic coverage.
    pub scope: Scope,
    /// Absolute expiry in Unix seconds, or no expiry until revoked.
    pub expires_at: Option<u64>,
}

impl GrantInput {
    fn validate(&mut self) -> Result<(), Error> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty()
            || self.name.chars().count() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err(invalid("Enter a name of 1 to 80 characters."));
        }
        if self.permissions.is_empty() {
            return Err(invalid("Choose at least one permission."));
        }
        if self.expires_at.is_some_and(|expiry| expiry <= now()) {
            return Err(invalid("Choose an expiry in the future."));
        }
        self.scope.validate()
    }
}

/// Public grant metadata. No key or verifier is exposed through this type.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grant {
    /// Stable identifier for trusted desktop management commands.
    pub id: String,
    /// Monotonic revision used to reject stale desktop edits.
    pub revision: u64,
    /// User-assigned label.
    pub name: String,
    /// Currently allowed operations.
    pub permissions: BTreeSet<Permission>,
    /// Approved traffic coverage.
    pub scope: Scope,
    /// Creation timestamp in Unix seconds.
    pub created_at: u64,
    /// Absolute expiration, if configured.
    pub expires_at: Option<u64>,
    /// Last successful authentication during this or a flushed prior run.
    pub last_used_at: Option<u64>,
    /// Whether the user permanently revoked this key.
    pub revoked: bool,
}

impl Grant {
    /// Rejects operations absent from this explicitly authored grant.
    /// # Errors
    /// Returns `permission_denied` when this capability has not been granted.
    pub fn require(&self, permission: Permission) -> Result<(), Error> {
        if self.permissions.contains(&permission) {
            Ok(())
        } else {
            Err(Error::new(
                "permission_denied",
                "This access key does not permit that operation. Edit its permissions in Transmog Settings → Integrations → MCP.",
            ))
        }
    }
}

/// One newly generated secret, returned only to the trusted creation dialog.
pub struct AccessKey(Zeroizing<String>);

impl AccessKey {
    /// Explicit access for the one-time key display. Never log or include it in agent results.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AccessKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AccessKey([redacted])")
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    grant: Grant,
    verifier: [u8; 32],
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Store {
    version: u32,
    enabled: bool,
    records: Vec<Record>,
}

/// Public settings state, independent of ordinary product preference writes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessStatus {
    /// Whether local agent access is explicitly enabled.
    pub enabled: bool,
    /// Grant metadata, including revoked entries for explicit removal.
    pub grants: Vec<Grant>,
}

/// Serialized durable access management. Corrupt state fails closed without loading older grants.
#[derive(Clone)]
pub struct AccessManager {
    state: Arc<Mutex<Store>>,
    path: Option<PathBuf>,
    changed: watch::Sender<u64>,
}

impl AccessManager {
    /// Loads one authoritative access store. A missing store starts disabled with no grants.
    /// # Errors
    /// Returns a safe persistence error for unreadable, oversized or corrupt existing state.
    pub fn open(path: Option<PathBuf>) -> Result<Self, Error> {
        let file = path.as_ref().map(std::fs::File::open).transpose();
        let state = match file {
            Ok(Some(file)) => {
                let mut bytes = Vec::new();
                file.take(MAX_STORE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| storage())?;
                if bytes.len() as u64 > MAX_STORE_BYTES {
                    return Err(storage());
                }
                let state: Store = serde_json::from_slice(&bytes).map_err(|_| storage())?;
                validate_store(&state)?;
                state
            }
            Ok(None) => Store {
                version: 1,
                ..Store::default()
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Store {
                version: 1,
                ..Store::default()
            },
            Err(_) => return Err(storage()),
        };
        let (changed, _) = watch::channel(0);
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            path,
            changed,
        })
    }

    /// Current safe metadata for the trusted settings view.
    pub fn status(&self) -> AccessStatus {
        let state = self.lock();
        AccessStatus {
            enabled: state.enabled,
            grants: state
                .records
                .iter()
                .map(|item| item.grant.clone())
                .collect(),
        }
    }

    /// Watches authority changes so active work can recheck or cancel.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Persists the master switch before publishing it. Disabling preserves key records.
    /// # Errors
    /// Returns a storage failure without changing the current authorization.
    pub fn set_enabled(&self, enabled: bool) -> Result<AccessStatus, Error> {
        self.mutate(|state| {
            state.enabled = enabled;
            Ok(())
        })?;
        Ok(self.status())
    }

    /// Creates an independent 256-bit secret after explicit desktop approval.
    /// # Errors
    /// Returns validation, entropy or persistence failure without publishing the key.
    pub fn create(&self, mut input: GrantInput) -> Result<(Grant, AccessKey), Error> {
        input.validate()?;
        let id = random(12)?;
        let key = AccessKey(Zeroizing::new(format!("tmk_{id}_{}", random(32)?)));
        let grant = Grant {
            id,
            revision: 1,
            name: input.name,
            permissions: input.permissions,
            scope: input.scope,
            created_at: now(),
            expires_at: input.expires_at,
            last_used_at: None,
            revoked: false,
        };
        let record = Record {
            grant: grant.clone(),
            verifier: Sha256::digest(key.expose().as_bytes()).into(),
        };
        self.mutate(|state| {
            if state.records.len() >= MAX_GRANTS { return Err(invalid("Remove an unused key before creating another. At most 64 keys can be retained.")); }
            state.records.push(record);
            Ok(())
        })?;
        Ok((grant, key))
    }

    /// Updates exactly the revision reviewed in the trusted desktop dialog.
    /// # Errors
    /// Returns validation, conflict or storage failure without changing the previous grant.
    pub fn update(
        &self,
        id: &str,
        revision: u64,
        mut input: GrantInput,
    ) -> Result<AccessStatus, Error> {
        input.validate()?;
        self.mutate(|state| {
            let record = find_record(state, id, revision)?;
            if record.grant.revoked {
                return Err(invalid(
                    "A revoked key cannot be re-enabled. Create a new key.",
                ));
            }
            record.grant.revision = record.grant.revision.checked_add(1).ok_or_else(storage)?;
            record.grant.name = input.name;
            record.grant.permissions = input.permissions;
            record.grant.scope = input.scope;
            record.grant.expires_at = input.expires_at;
            Ok(())
        })?;
        Ok(self.status())
    }

    /// Permanently revokes one key, or all keys when no ID is supplied.
    /// # Errors
    /// Returns a stale-revision or storage failure without restoring any older grant.
    pub fn revoke(&self, target: Option<(&str, u64)>) -> Result<AccessStatus, Error> {
        self.mutate(|state| {
            if let Some((id, revision)) = target {
                let record = find_record(state, id, revision)?;
                record.grant.revoked = true;
                record.grant.revision = record.grant.revision.checked_add(1).ok_or_else(storage)?;
            } else {
                for record in &mut state.records {
                    record.grant.revoked = true;
                    record.grant.revision =
                        record.grant.revision.checked_add(1).ok_or_else(storage)?;
                }
            }
            Ok(())
        })?;
        Ok(self.status())
    }

    /// Removes only a revoked key's management record; its secret remains invalid.
    /// # Errors
    /// Rejects stale or still-active records and persistence failures.
    pub fn remove(&self, id: &str, revision: u64) -> Result<AccessStatus, Error> {
        self.mutate(|state| {
            if !find_record(state, id, revision)?.grant.revoked {
                return Err(invalid("Revoke this key before removing its record."));
            }
            state.records.retain(|record| record.grant.id != id);
            Ok(())
        })?;
        Ok(self.status())
    }

    /// Checks a caller-supplied secret on every operation. Names and connection state never authenticate.
    /// # Errors
    /// Returns disabled or unauthorized without revealing whether an individual key ID exists.
    pub fn authenticate(&self, key: &str) -> Result<Grant, Error> {
        let mut state = self.lock();
        if !state.enabled {
            return Err(Error::new(
                "mcp_disabled",
                "Enable Allow MCP connections in Transmog Settings → Integrations → MCP.",
            ));
        }
        if key.len() > 128 || !key.starts_with("tmk_") {
            return Err(unauthorized());
        }
        let verifier: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        let record = state
            .records
            .iter_mut()
            .find(|record| boring::memcmp::eq(&record.verifier, &verifier))
            .ok_or_else(unauthorized)?;
        if record.grant.revoked
            || record
                .grant
                .expires_at
                .is_some_and(|expiry| expiry <= now())
        {
            return Err(unauthorized());
        }
        record.grant.last_used_at = Some(now());
        Ok(record.grant.clone())
    }

    /// Flushes last-use metadata without changing permission generations.
    /// # Errors
    /// Returns a safe persistence failure.
    pub fn flush(&self) -> Result<(), Error> {
        self.persist(&self.lock())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn mutate<T>(&self, change: impl FnOnce(&mut Store) -> Result<T, Error>) -> Result<T, Error> {
        let mut state = self.lock();
        let mut next = state.clone();
        let result = change(&mut next)?;
        self.persist(&next)?;
        *state = next;
        self.changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(result)
    }

    fn persist(&self, state: &Store) -> Result<(), Error> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(storage)?;
        std::fs::create_dir_all(parent).map_err(|_| storage())?;
        let bytes = serde_json::to_vec(state).map_err(|_| storage())?;
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(storage());
        }
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| storage())?;
        file.write_all(&bytes)
            .and_then(|()| file.as_file().sync_all())
            .map_err(|_| storage())?;
        file.persist(path).map_err(|_| storage())?;
        Ok(())
    }
}

fn validate_store(state: &Store) -> Result<(), Error> {
    if state.version != 1 || state.records.len() > MAX_GRANTS {
        return Err(storage());
    }
    let mut ids = BTreeSet::new();
    for record in &state.records {
        let grant = &record.grant;
        let mut input = GrantInput {
            name: grant.name.clone(),
            permissions: grant.permissions.clone(),
            scope: grant.scope.clone(),
            expires_at: None,
        };
        input.validate().map_err(|_| storage())?;
        if grant.id.is_empty()
            || grant.id.len() > 64
            || grant.revision == 0
            || !ids.insert(grant.id.clone())
            || input.scope != grant.scope
            || input.name != grant.name
        {
            return Err(storage());
        }
    }
    Ok(())
}

fn find_record<'a>(state: &'a mut Store, id: &str, revision: u64) -> Result<&'a mut Record, Error> {
    state
        .records
        .iter_mut()
        .find(|record| record.grant.id == id && record.grant.revision == revision)
        .ok_or_else(|| {
            Error::new(
                "conflict",
                "This key changed. Refresh its settings and try again.",
            )
        })
}

pub(crate) fn random(length: usize) -> Result<String, Error> {
    let mut bytes = Zeroizing::new(vec![0_u8; length]);
    getrandom::fill(&mut bytes).map_err(|_| {
        Error::new(
            "unavailable",
            "A secure random value could not be generated.",
        )
    })?;
    Ok(URL_SAFE_NO_PAD.encode(&*bytes))
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn invalid(message: &str) -> Error {
    Error::new("invalid_input", message)
}
fn unauthorized() -> Error {
    Error::new(
        "unauthorized",
        "Supply a valid Transmog access key through TRANSMOG_ACCESS_KEY. Create or replace it in Settings → Integrations → MCP.",
    )
}
fn storage() -> Error {
    Error::new(
        "access_store_unavailable",
        "MCP access settings could not be read or saved. Access remains unavailable until the settings file is repaired.",
    )
}

#[cfg(test)]
mod tests;
