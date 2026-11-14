#![deny(missing_docs)]

//! Transactional current-user Windows proxy and certificate integration.
//!
//! System-proxy changes are protected by an atomically created recovery
//! journal. Certificate trust is deliberately a separate, explicit workflow.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Seek as _, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, Weak},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use transmog_session::{HostIntegration, HostIntegrationError, HostRestoreToken};

mod ownership;
mod recovery_task;
use ownership::{SharedProxyOwner, lock_file, read_json, replace_json, same_path, write_new_json};
pub use recovery_task::{ProxyRecoveryTask, ScheduledProxyRecovery, recovery_command_exit_code};

/// Exact current-user Internet Settings values managed by the adapter.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    /// Whether the proxy is enabled; `None` means the value did not exist.
    pub enabled: Option<u32>,
    /// Exact prior `ProxyServer` string, if present.
    pub server: Option<String>,
    /// Exact prior `ProxyOverride` string, if present.
    pub bypass: Option<String>,
}

/// Injectable operating-system boundary used by deterministic tests.
pub trait ProxySettingsBackend: Send + Sync {
    /// Reads the exact current-user state.
    ///
    /// # Errors
    /// Returns a bounded registry or command failure.
    fn read(&self) -> Result<ProxySettings, WindowsHostError>;
    /// Replaces the managed values, including deleting absent values.
    ///
    /// # Errors
    /// Returns a bounded registry or command failure.
    fn write(&self, settings: &ProxySettings) -> Result<(), WindowsHostError>;
    /// Notifies WinINet/WebView consumers that settings changed.
    ///
    /// # Errors
    /// Returns a bounded notification failure.
    fn notify_changed(&self) -> Result<(), WindowsHostError>;

    /// Restores managed values while preserving later changes by another actor.
    /// The completion flag distinguishes an interrupted apply from later edits.
    ///
    /// # Errors
    /// Returns a read, write, or notification failure that remains retryable.
    fn restore_owned(
        &self,
        prior: &ProxySettings,
        applied: Option<&ProxySettings>,
        application_complete: bool,
    ) -> Result<(), WindowsHostError> {
        let current = self.read()?;
        let restore = restored_settings(prior, applied, &current, application_complete);
        if restore != current {
            self.write(&restore)?;
        }
        self.notify_changed()
    }
}

/// Production backend implemented with inbox Windows command-line facilities.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProxyBackend;

impl ProxySettingsBackend for SystemProxyBackend {
    fn read(&self) -> Result<ProxySettings, WindowsHostError> {
        run_settings_script(READ_SETTINGS_SCRIPT, None)
    }

    fn write(&self, settings: &ProxySettings) -> Result<(), WindowsHostError> {
        let json = serde_json::to_string(settings).map_err(WindowsHostError::Journal)?;
        run_script(WRITE_SETTINGS_SCRIPT, Some(&json)).map(|_| ())
    }

    fn notify_changed(&self) -> Result<(), WindowsHostError> {
        run_script(NOTIFY_SETTINGS_SCRIPT, None).map(|_| ())
    }

    fn restore_owned(
        &self,
        prior: &ProxySettings,
        applied: Option<&ProxySettings>,
        application_complete: bool,
    ) -> Result<(), WindowsHostError> {
        let input = serde_json::json!({ "prior": prior, "applied": applied, "applicationComplete": application_complete }).to_string();
        // Keep the current-state check and the write in one process. Starting a
        // separate PowerShell writer would widen the race with VPN/user edits.
        let script = format!(
            "$snapshot = $env:TRANSMOG_INPUT | ConvertFrom-Json\n\
             $current = (& {{ {READ_SETTINGS_SCRIPT} }}) | ConvertFrom-Json\n\
             {RESTORE_PLAN_SCRIPT}\n\
             if ($restore.enabled -cne $current.enabled -or $restore.server -cne $current.server -or $restore.bypass -cne $current.bypass) {{\n\
               $env:TRANSMOG_INPUT = $restore | ConvertTo-Json -Compress\n\
               {WRITE_SETTINGS_SCRIPT}\n\
             }}\n\
             {NOTIFY_SETTINGS_SCRIPT}"
        );
        run_script(&script, Some(&input)).map(|_| ())
    }
}

fn restored_settings(
    prior: &ProxySettings,
    applied: Option<&ProxySettings>,
    current: &ProxySettings,
    application_complete: bool,
) -> ProxySettings {
    let mut restore = prior.clone();
    if let Some(applied) = applied {
        if !application_complete
            && current.server == prior.server
            && current.enabled == applied.enabled
            && current.bypass == prior.bypass
        {
            // Applying writes enabled first. Before the completion marker,
            // this is a half-applied route, not an explicit later selection.
            return prior.clone();
        }
        if current.server != applied.server {
            // Even reselecting the prior endpoint can be an explicit new choice.
            // A partial restore writes enabled and server before bypass, so only
            // finish that bypass when the complete route already matches prior.
            let mut restore = current.clone();
            if current.server == prior.server
                && current.enabled == prior.enabled
                && current.bypass == applied.bypass
            {
                restore.bypass.clone_from(&prior.bypass);
            }
            return restore;
        }
        if current.enabled != applied.enabled && current.enabled != prior.enabled {
            restore.enabled = current.enabled;
        }
        if current.bypass != applied.bypass && current.bypass != prior.bypass {
            restore.bypass.clone_from(&current.bypass);
        }
    }
    restore
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryJournal {
    schema: u32,
    prior: ProxySettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied: Option<ProxySettings>,
    #[serde(default)]
    application_complete: bool,
}

#[derive(Debug)]
struct RestoreState {
    prior: ProxySettings,
    applied: ProxySettings,
    lease: Mutex<Option<Vec<File>>>,
}

/// Transactional current-user system-proxy integration.
#[derive(Clone)]
pub struct WindowsProxyIntegration {
    backend: Arc<dyn ProxySettingsBackend>,
    journal_path: PathBuf,
    recovery_task: Option<Arc<dyn ProxyRecoveryTask>>,
    diagnostic: Arc<dyn Fn(&str) + Send + Sync>,
    active: Arc<Mutex<Weak<RestoreState>>>,
    shared_owner: Option<SharedProxyOwner>,
}

impl std::fmt::Debug for WindowsProxyIntegration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowsProxyIntegration")
            .field("journal_path", &self.journal_path)
            .finish_non_exhaustive()
    }
}

impl WindowsProxyIntegration {
    /// Creates the production current-user adapter.
    pub fn system(journal_path: impl Into<PathBuf>) -> Self {
        let journal_path = journal_path.into();
        let mut integration = Self::new(Arc::new(SystemProxyBackend), &journal_path);
        if cfg!(windows) {
            integration.shared_owner = Some(SharedProxyOwner::current_user());
        }
        match std::env::current_exe() {
            Ok(executable) => {
                integration.recovery_task = Some(Arc::new(ScheduledProxyRecovery::new(
                    journal_path,
                    executable,
                )));
            }
            Err(error) => (integration.diagnostic)(&format!(
                "Windows proxy login recovery is unavailable: {error}"
            )),
        }
        integration
    }

    /// Creates an adapter around an injected backend.
    pub fn new(backend: Arc<dyn ProxySettingsBackend>, journal_path: impl Into<PathBuf>) -> Self {
        Self {
            backend,
            journal_path: journal_path.into(),
            recovery_task: None,
            diagnostic: Arc::new(|message| eprintln!("{message}")),
            active: Arc::new(Mutex::new(Weak::new())),
            shared_owner: None,
        }
    }

    /// Adds best-effort login recovery to this adapter.
    #[must_use]
    pub fn with_recovery_task(mut self, task: Arc<dyn ProxyRecoveryTask>) -> Self {
        self.recovery_task = Some(task);
        self
    }

    /// Routes nonfatal task registration and cleanup failures to host diagnostics.
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostic: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.diagnostic = Arc::new(diagnostic);
        self
    }

    /// Returns whether a prior process left a recovery journal.
    pub fn recovery_pending(&self) -> bool {
        self.journal_pending()
            || self
                .shared_owner
                .as_ref()
                .is_some_and(SharedProxyOwner::recovery_pending)
    }

    fn journal_pending(&self) -> bool {
        // An unreadable or malformed path is pending recovery, not proof that
        // the snapshot disappeared. Never disarm its task on an attribute error.
        self.journal_path.try_exists().unwrap_or(true)
    }

    /// Restores a crash journal before a new proxy run.
    ///
    /// # Errors
    ///
    /// Returns a bounded I/O, journal, registry, or notification failure. A
    /// failed recovery preserves the journal so retrying is safe.
    /// A live proxy's lease prevents recovery until that run has stopped.
    pub fn recover_pending(&self) -> Result<bool, WindowsHostError> {
        let _lease = self.acquire_lease()?;
        let recovered_previous = self.recover_previous_owner()?;
        if !self.journal_pending() {
            self.remove_recovery_task();
            return Ok(recovered_previous);
        }
        let journal: RecoveryJournal =
            read_json(&self.journal_path)?.ok_or(WindowsHostError::InvalidJournal)?;
        if journal.schema != 1 {
            return Err(WindowsHostError::InvalidJournal);
        }
        self.restore_owned_settings(
            &journal.prior,
            journal.applied.as_ref(),
            journal.application_complete,
        )?;
        remove_journal(&self.journal_path)?;
        self.clear_owner()?;
        self.remove_recovery_task();
        Ok(true)
    }

    /// Restores this adapter's active run when the host is exiting.
    ///
    /// Unlike background recovery, this may restore a live run owned by this
    /// adapter. An idle adapter leaves another profile's live run alone. The
    /// host must be committed to exiting before calling it.
    ///
    /// # Errors
    /// Returns a retryable journal, registry, notification or I/O failure.
    pub fn recover_for_exit(&self) -> Result<(), WindowsHostError> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade();
        if let Some(active) = active {
            self.restore_state(&active)
        } else {
            match self.recover_pending() {
                // Another profile can own the current-user lease even when
                // this host has no state to restore. That must not block exit.
                Err(WindowsHostError::ProxyInUse) if !self.journal_pending() => Ok(()),
                result => result.map(|_| ()),
            }
        }
    }

    fn restore_state(&self, state: &RestoreState) -> Result<(), WindowsHostError> {
        let mut lease = state
            .lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lease.is_none() {
            return Ok(());
        }
        self.restore_owned_settings(&state.prior, Some(&state.applied), true)?;
        remove_journal(&self.journal_path)?;
        self.clear_owner()?;
        self.remove_recovery_task();
        *lease = None;
        Ok(())
    }

    fn acquire_lease(&self) -> Result<Vec<File>, WindowsHostError> {
        let mut leases = Vec::new();
        if let Some(owner) = &self.shared_owner {
            leases.push(owner.lease()?);
        }
        leases.push(lock_file(&self.journal_path.with_extension("lock"))?);
        Ok(leases)
    }

    fn recover_previous_owner(&self) -> Result<bool, WindowsHostError> {
        let Some(owner) = &self.shared_owner else {
            return Ok(false);
        };
        let Some(journal) = owner.pending()? else {
            return Ok(false);
        };
        if same_path(&journal, &self.journal_path) {
            if !self.journal_pending() {
                self.clear_owner()?;
            }
            return Ok(false);
        }
        // The shared lease is already held. Only acquire the previous journal's
        // own lease here, and recover it before taking another proxy snapshot.
        let mut previous = Self::new(Arc::clone(&self.backend), &journal);
        previous.diagnostic = Arc::clone(&self.diagnostic);
        if cfg!(windows)
            && self.recovery_task.is_some()
            && let Ok(executable) = std::env::current_exe()
        {
            previous.recovery_task =
                Some(Arc::new(ScheduledProxyRecovery::new(&journal, executable)));
        }
        let recovered = previous.recover_pending()?;
        owner.clear(&journal)?;
        Ok(recovered)
    }

    fn clear_owner(&self) -> Result<(), WindowsHostError> {
        if let Some(owner) = &self.shared_owner {
            let registered_journal = owner.pending()?;
            owner.clear(&self.journal_path)?;
            // The scheduler name contains the path used at registration.
            // Retain it until cleanup even when recovery used a junction alias.
            if let Some(registered_journal) = registered_journal
                && registered_journal != self.journal_path
            {
                self.remove_recovery_task_for_journal(&registered_journal);
            }
        }
        Ok(())
    }

    fn remove_recovery_task(&self) {
        self.remove_recovery_task_for_journal(&self.journal_path);
    }

    fn remove_recovery_task_for_journal(&self, journal: &Path) {
        if let Some(task) = &self.recovery_task
            && let Err(error) = task.remove_for_journal(journal)
        {
            (self.diagnostic)(&format!(
                "Windows proxy login recovery task cleanup failed: {error}"
            ));
        }
    }

    fn restore_owned_settings(
        &self,
        prior: &ProxySettings,
        applied: Option<&ProxySettings>,
        application_complete: bool,
    ) -> Result<(), WindowsHostError> {
        // Notification must be retried even when an earlier attempt restored
        // the registry but failed to refresh WinINet's cached settings.
        self.backend
            .restore_owned(prior, applied, application_complete)
    }
}

impl HostIntegration for WindowsProxyIntegration {
    fn apply(&self, endpoint: SocketAddr) -> Result<HostRestoreToken, HostIntegrationError> {
        // Wildcard listeners still route this computer through its loopback interface.
        let endpoint = if endpoint.ip().is_unspecified() {
            SocketAddr::new(
                if endpoint.is_ipv4() {
                    IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                } else {
                    IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
                },
                endpoint.port(),
            )
        } else {
            endpoint
        };
        if !endpoint.ip().is_loopback() {
            return Err(host_error("system proxy endpoint must be loopback"));
        }
        let lease = self
            .acquire_lease()
            .map_err(|error| to_host_error(&error))?;
        self.recover_previous_owner()
            .map_err(|error| to_host_error(&error))?;
        if self.recovery_pending() {
            return Err(host_error("a Windows proxy recovery journal is pending"));
        }
        let prior = self.backend.read().map_err(|error| to_host_error(&error))?;
        let host = match endpoint.ip() {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        let desired = ProxySettings {
            enabled: Some(1),
            server: Some(format!(
                "http={host}:{};https={host}:{}",
                endpoint.port(),
                endpoint.port()
            )),
            bypass: Some("<local>".to_owned()),
        };
        let mut journal = write_journal(&self.journal_path, &prior, &desired)
            .map_err(|error| to_host_error(&error))?;
        if let Some(owner) = &self.shared_owner
            && let Err(error) = owner.claim(&self.journal_path)
        {
            let _ = remove_journal(&self.journal_path);
            return Err(to_host_error(&error));
        }
        if let Some(task) = &self.recovery_task
            && let Err(error) = task.register()
        {
            (self.diagnostic)(&format!(
                "Windows proxy login recovery task registration failed: {error}"
            ));
        }
        if let Err(error) = self
            .backend
            .write(&desired)
            .and_then(|()| self.backend.notify_changed())
            .and_then(|()| {
                journal.application_complete = true;
                replace_json(&self.journal_path, &journal)
            })
        {
            if self
                .restore_owned_settings(&prior, Some(&desired), false)
                .is_ok()
                && remove_journal(&self.journal_path).is_ok()
                && self.clear_owner().is_ok()
            {
                self.remove_recovery_task();
            }
            return Err(to_host_error(&error));
        }
        let state = Arc::new(RestoreState {
            prior,
            applied: desired,
            lease: Mutex::new(Some(lease)),
        });
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&state);
        Ok(HostRestoreToken::new(state))
    }

    fn restore(&self, token: &HostRestoreToken) -> Result<(), HostIntegrationError> {
        let state = token
            .downcast_ref::<Arc<RestoreState>>()
            .ok_or_else(|| host_error("Windows proxy restore token type mismatch"))?;
        self.restore_state(state)
            .map_err(|error| to_host_error(&error))
    }
}

/// Explicit certificate-store action outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CertificateAction {
    /// The requested certificate is now trusted.
    Installed,
    /// The exact requested certificate was removed.
    Removed,
}

/// Explicit current-user root certificate workflow.
#[derive(Clone, Copy, Debug, Default)]
pub struct CurrentUserCertificateStore;

impl CurrentUserCertificateStore {
    /// Installs the public certificate after verifying the expected SHA-256.
    ///
    /// This may display an operating-system consent dialog. It is never called
    /// as a side effect of proxy start.
    ///
    /// # Errors
    /// Returns a path, certificate identity, store, or command failure.
    pub fn install(
        &self,
        certificate_path: &Path,
        expected_sha256: &str,
    ) -> Result<CertificateAction, WindowsHostError> {
        validate_thumbprint(expected_sha256)?;
        let payload = serde_json::json!({
            "path": certificate_path,
            "thumbprint": expected_sha256,
        });
        run_script(CERT_INSTALL_SCRIPT, Some(&payload.to_string()))?;
        Ok(CertificateAction::Installed)
    }

    /// Removes only the exact expected SHA-256 certificate from current-user roots.
    ///
    /// # Errors
    /// Returns an identity, store, or command failure.
    pub fn remove(&self, expected_sha256: &str) -> Result<CertificateAction, WindowsHostError> {
        validate_thumbprint(expected_sha256)?;
        run_script(CERT_REMOVE_SCRIPT, Some(expected_sha256))?;
        Ok(CertificateAction::Removed)
    }

    /// Checks for the exact certificate SHA-256 in current-user roots.
    ///
    /// # Errors
    /// Returns an identity, store, or command failure.
    pub fn contains(&self, expected_sha256: &str) -> Result<bool, WindowsHostError> {
        validate_thumbprint(expected_sha256)?;
        Ok(run_script(CERT_CONTAINS_SCRIPT, Some(expected_sha256))?.trim() == "true")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CertificateOwnershipRecord {
    schema: u32,
    sha256: String,
}

/// Durable exact-thumbprint ownership record for app-created certificate
/// material, whether or not it is currently present in current-user roots.
#[derive(Clone, Debug)]
pub struct OwnedCertificateRegistry {
    path: PathBuf,
}

impl OwnedCertificateRegistry {
    /// Creates a registry at an application-owned path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Records the exact app-created certificate before trust-store mutation.
    ///
    /// Repeating the same claim is idempotent. A different existing claim
    /// fails closed so no certificate can become silently orphaned.
    ///
    /// # Errors
    /// Returns a thumbprint, ownership conflict, serialization, or I/O error.
    pub fn claim(&self, sha256: &str) -> Result<bool, WindowsHostError> {
        validate_thumbprint(sha256)?;
        if let Some(existing) = self.owned_thumbprint()? {
            if existing.eq_ignore_ascii_case(sha256) {
                return Ok(false);
            }
            return Err(WindowsHostError::CertificateOwnershipConflict);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(WindowsHostError::Io)?;
        }
        let bytes = serde_json::to_vec(&CertificateOwnershipRecord {
            schema: 1,
            sha256: sha256.to_ascii_uppercase(),
        })
        .map_err(WindowsHostError::Journal)?;
        let temp = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(WindowsHostError::Io)?;
        let result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| fs::rename(&temp, &self.path));
        if let Err(error) = result {
            let _ = fs::remove_file(temp);
            return Err(WindowsHostError::Io(error));
        }
        Ok(true)
    }

    /// Returns the exact owned certificate identity, if one is recorded.
    ///
    /// # Errors
    /// Returns a malformed, oversized, unsupported, or unreadable record.
    pub fn owned_thumbprint(&self) -> Result<Option<String>, WindowsHostError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(WindowsHostError::Io(error)),
        };
        if bytes.len() > 4 * 1024 {
            return Err(WindowsHostError::InvalidCertificateOwnership);
        }
        let record: CertificateOwnershipRecord = serde_json::from_slice(&bytes)
            .map_err(|_| WindowsHostError::InvalidCertificateOwnership)?;
        if record.schema != 1 || validate_thumbprint(&record.sha256).is_err() {
            return Err(WindowsHostError::InvalidCertificateOwnership);
        }
        Ok(Some(record.sha256))
    }

    /// Clears the ownership record only when it identifies the exact expected
    /// certificate.
    ///
    /// # Errors
    /// Returns a thumbprint, ownership conflict, malformed record, or I/O error.
    pub fn clear(&self, sha256: &str) -> Result<(), WindowsHostError> {
        validate_thumbprint(sha256)?;
        match self.owned_thumbprint()? {
            Some(existing) if existing.eq_ignore_ascii_case(sha256) => remove_journal(&self.path),
            Some(_) => Err(WindowsHostError::CertificateOwnershipConflict),
            None => Ok(()),
        }
    }
}

/// Applies a current-user-only Windows ACL to app-owned private key files.
#[derive(Clone, Copy, Debug, Default)]
pub struct CurrentUserKeyProtection;

impl CurrentUserKeyProtection {
    /// Protects an app-owned directory before creating private material inside
    /// it. Child files and directories inherit access for only the current user.
    ///
    /// # Errors
    /// Returns an OS path or ACL failure.
    pub fn protect_directory(&self, directory: &Path) -> Result<(), WindowsHostError> {
        let payload = serde_json::json!({ "path": directory });
        run_script(DIRECTORY_PROTECTION_SCRIPT, Some(&payload.to_string())).map(|_| ())
    }

    /// Removes inherited access and grants full control only to the current
    /// user for one existing private-key file.
    ///
    /// # Errors
    /// Returns a path, ACL, or Windows command failure.
    pub fn protect(&self, private_key_path: &Path) -> Result<(), WindowsHostError> {
        let payload = serde_json::json!({ "path": private_key_path });
        run_script(KEY_PROTECTION_SCRIPT, Some(&payload.to_string())).map(|_| ())
    }
}

/// Bounded Windows adapter failure.
#[derive(Debug, Error)]
pub enum WindowsHostError {
    /// The adapter was used on a non-Windows host.
    #[error("Windows host integration is unavailable on this platform")]
    UnsupportedPlatform,
    /// A Windows command failed.
    #[error("Windows host command failed")]
    CommandFailed,
    /// A best-effort Windows command exceeded its deadline.
    #[error("Windows host command timed out")]
    CommandTimedOut,
    /// A live proxy or another recovery process owns this journal.
    #[error("Windows proxy settings are in use by another capture or recovery operation")]
    ProxyInUse,
    /// The crash journal was malformed or unsupported.
    #[error("Windows proxy recovery journal is invalid")]
    InvalidJournal,
    /// File-system operation failed.
    #[error("Windows host I/O failed: {0}")]
    Io(#[source] io::Error),
    /// Journal serialization failed.
    #[error("Windows host journal serialization failed: {0}")]
    Journal(#[source] serde_json::Error),
    /// A certificate thumbprint was not a canonical SHA-256 value.
    #[error("certificate thumbprint must be 64 hexadecimal characters")]
    InvalidThumbprint,
    /// An ownership record refers to a different exact certificate.
    #[error("a different app-owned certificate is already recorded")]
    CertificateOwnershipConflict,
    /// A certificate ownership record was malformed or unsupported.
    #[error("certificate ownership record is invalid")]
    InvalidCertificateOwnership,
}

fn write_journal(
    path: &Path,
    prior: &ProxySettings,
    applied: &ProxySettings,
) -> Result<RecoveryJournal, WindowsHostError> {
    let journal = RecoveryJournal {
        schema: 1,
        prior: prior.clone(),
        applied: Some(applied.clone()),
        application_complete: false,
    };
    write_new_json(path, &journal)?;
    Ok(journal)
}

fn remove_journal(path: &Path) -> Result<(), WindowsHostError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(WindowsHostError::Io(error)),
    }
}

fn validate_thumbprint(value: &str) -> Result<(), WindowsHostError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(WindowsHostError::InvalidThumbprint)
    }
}

fn run_settings_script(
    script: &str,
    input: Option<&str>,
) -> Result<ProxySettings, WindowsHostError> {
    let output = run_script(script, input)?;
    serde_json::from_str(output.trim()).map_err(WindowsHostError::Journal)
}

fn script_command(script: &str, input: Option<&str>) -> Result<Command, WindowsHostError> {
    if !cfg!(windows) {
        return Err(WindowsHostError::UnsupportedPlatform);
    }
    let mut command = Command::new("powershell.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
    command.arg(format!(
        "$ErrorActionPreference = 'Stop';\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false);\n{script}"
    ));
    if let Some(input) = input {
        command.env("TRANSMOG_INPUT", input);
    }
    Ok(command)
}

fn run_script(script: &str, input: Option<&str>) -> Result<String, WindowsHostError> {
    let mut command = script_command(script, input)?;
    let output = command.output().map_err(WindowsHostError::Io)?;
    if !output.status.success() {
        return Err(WindowsHostError::CommandFailed);
    }
    String::from_utf8(output.stdout).map_err(|_| WindowsHostError::CommandFailed)
}

fn run_script_bounded(
    script: &str,
    input: Option<&str>,
    timeout: std::time::Duration,
) -> Result<String, WindowsHostError> {
    use std::{process::Stdio, time::Instant};
    // An anonymous temporary file avoids pipe backpressure and reader threads
    // that could outlive a timed-out RPC to Task Scheduler.
    let mut output = tempfile::tempfile().map_err(WindowsHostError::Io)?;
    let mut command = script_command(script, input)?;
    command
        .stdout(output.try_clone().map_err(WindowsHostError::Io)?)
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(WindowsHostError::Io)?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => WindowsHostError::Io(error),
                    _ => WindowsHostError::CommandTimedOut,
                });
            }
        }
    };
    if !status.success() {
        return Err(WindowsHostError::CommandFailed);
    }
    output.rewind().map_err(WindowsHostError::Io)?;
    let mut bytes = Vec::new();
    output
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(WindowsHostError::Io)?;
    if bytes.len() > 1024 * 1024 {
        return Err(WindowsHostError::CommandFailed);
    }
    String::from_utf8(bytes).map_err(|_| WindowsHostError::CommandFailed)
}

fn to_host_error(error: &WindowsHostError) -> HostIntegrationError {
    host_error(error.to_string())
}

fn host_error(message: impl Into<String>) -> HostIntegrationError {
    HostIntegrationError::new(message.into())
}

const RESTORE_PLAN_SCRIPT: &str = r"
$restore = $snapshot.prior
if ($null -ne $snapshot.applied) {
    if (-not $snapshot.applicationComplete -and $current.server -ceq $snapshot.prior.server -and $current.enabled -ceq $snapshot.applied.enabled -and $current.bypass -ceq $snapshot.prior.bypass) {
        $restore = $snapshot.prior
    } elseif ($current.server -cne $snapshot.applied.server) {
        $restore = $current.PSObject.Copy()
        if ($current.server -ceq $snapshot.prior.server -and $current.enabled -ceq $snapshot.prior.enabled -and $current.bypass -ceq $snapshot.applied.bypass) {
            $restore.bypass = $snapshot.prior.bypass
        }
    } else {
        if ($current.enabled -cne $snapshot.applied.enabled -and $current.enabled -cne $snapshot.prior.enabled) { $restore.enabled = $current.enabled }
        if ($current.bypass -cne $snapshot.applied.bypass -and $current.bypass -cne $snapshot.prior.bypass) { $restore.bypass = $current.bypass }
    }
}
";

const READ_SETTINGS_SCRIPT: &str = r"
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Internet Settings')
$names = @($key.GetValueNames())
$result = @{
  enabled = if ($names -contains 'ProxyEnable') { [int]$key.GetValue('ProxyEnable') } else { $null }
  server = if ($names -contains 'ProxyServer') { [string]$key.GetValue('ProxyServer') } else { $null }
  bypass = if ($names -contains 'ProxyOverride') { [string]$key.GetValue('ProxyOverride') } else { $null }
}
$key.Dispose()
$result | ConvertTo-Json -Compress
";

const WRITE_SETTINGS_SCRIPT: &str = r"
# Keep this order: applying-phase recovery recognizes the enabled-only prefix.
$s = $env:TRANSMOG_INPUT | ConvertFrom-Json
$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\Microsoft\Windows\CurrentVersion\Internet Settings')
if ($null -eq $s.enabled) { $key.DeleteValue('ProxyEnable', $false) } else { $key.SetValue('ProxyEnable', [int]$s.enabled, [Microsoft.Win32.RegistryValueKind]::DWord) }
if ($null -eq $s.server) { $key.DeleteValue('ProxyServer', $false) } else { $key.SetValue('ProxyServer', [string]$s.server, [Microsoft.Win32.RegistryValueKind]::String) }
if ($null -eq $s.bypass) { $key.DeleteValue('ProxyOverride', $false) } else { $key.SetValue('ProxyOverride', [string]$s.bypass, [Microsoft.Win32.RegistryValueKind]::String) }
$key.Dispose()
";

const NOTIFY_SETTINGS_SCRIPT: &str = r#"
Add-Type -Namespace Transmog -Name WinInet -MemberDefinition '[DllImport("wininet.dll", SetLastError=true)] public static extern bool InternetSetOption(IntPtr hInternet, int option, IntPtr buffer, int length);'
if (-not [Transmog.WinInet]::InternetSetOption([IntPtr]::Zero, 39, [IntPtr]::Zero, 0)) { throw 'settings changed notification failed' }
if (-not [Transmog.WinInet]::InternetSetOption([IntPtr]::Zero, 37, [IntPtr]::Zero, 0)) { throw 'settings refresh notification failed' }
"#;

const CERT_INSTALL_SCRIPT: &str = r"
$i = $env:TRANSMOG_INPUT | ConvertFrom-Json
$cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new([string]$i.path)
$hash = [BitConverter]::ToString($cert.GetCertHash([System.Security.Cryptography.HashAlgorithmName]::SHA256)).Replace('-','')
if ($hash -ne ([string]$i.thumbprint).ToUpperInvariant()) { throw 'certificate SHA-256 mismatch' }
$store = [System.Security.Cryptography.X509Certificates.X509Store]::new('Root','CurrentUser')
$store.Open('ReadWrite'); $store.Add($cert); $store.Close()
";

const CERT_REMOVE_SCRIPT: &str = r"
$expected = $env:TRANSMOG_INPUT.ToUpperInvariant()
$store = [System.Security.Cryptography.X509Certificates.X509Store]::new('Root','CurrentUser')
$store.Open('ReadWrite')
@($store.Certificates) | Where-Object { [BitConverter]::ToString($_.GetCertHash([System.Security.Cryptography.HashAlgorithmName]::SHA256)).Replace('-','') -eq $expected } | ForEach-Object { $store.Remove($_) }
$store.Close()
";

const CERT_CONTAINS_SCRIPT: &str = r"
$expected = $env:TRANSMOG_INPUT.ToUpperInvariant()
$store = [System.Security.Cryptography.X509Certificates.X509Store]::new('Root','CurrentUser')
$store.Open('ReadOnly')
$found = @($store.Certificates) | Where-Object { [BitConverter]::ToString($_.GetCertHash([System.Security.Cryptography.HashAlgorithmName]::SHA256)).Replace('-','') -eq $expected } | Select-Object -First 1
$store.Close()
if ($null -eq $found) { 'false' } else { 'true' }
";

const KEY_PROTECTION_SCRIPT: &str = r"
$i = $env:TRANSMOG_INPUT | ConvertFrom-Json
$path = [System.IO.Path]::GetFullPath([string]$i.path)
if (-not [System.IO.File]::Exists($path)) { throw 'private key does not exist' }
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [System.Security.AccessControl.FileSecurity]::new()
$acl.SetAccessRuleProtection($true, $false)
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new($identity, [System.Security.AccessControl.FileSystemRights]::FullControl, [System.Security.AccessControl.AccessControlType]::Allow)
$acl.AddAccessRule($rule)
[System.IO.File]::SetAccessControl($path, $acl)
";

const DIRECTORY_PROTECTION_SCRIPT: &str = r"
$i = $env:TRANSMOG_INPUT | ConvertFrom-Json
$path = [System.IO.Path]::GetFullPath([string]$i.path)
if (-not [System.IO.Directory]::Exists($path)) { throw 'owned directory does not exist' }
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [System.Security.AccessControl.DirectorySecurity]::new()
$acl.SetAccessRuleProtection($true, $false)
$inherit = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new($identity, [System.Security.AccessControl.FileSystemRights]::FullControl, $inherit, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow)
$acl.AddAccessRule($rule)
[System.IO.Directory]::SetAccessControl($path, $acl)
";

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug)]
    struct FakeBackend {
        value: Mutex<ProxySettings>,
        writes: Mutex<Vec<ProxySettings>>,
        fail_notify: Mutex<u32>,
    }

    impl ProxySettingsBackend for FakeBackend {
        fn read(&self) -> Result<ProxySettings, WindowsHostError> {
            Ok(self.value.lock().unwrap().clone())
        }

        fn write(&self, settings: &ProxySettings) -> Result<(), WindowsHostError> {
            *self.value.lock().unwrap() = settings.clone();
            self.writes.lock().unwrap().push(settings.clone());
            Ok(())
        }

        fn notify_changed(&self) -> Result<(), WindowsHostError> {
            let mut failures = self.fail_notify.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                Err(WindowsHostError::CommandFailed)
            } else {
                Ok(())
            }
        }
    }

    fn temp_journal(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "transmog-host-{label}-{}-{}.json",
            std::process::id(),
            label
        ))
    }

    fn adapter(label: &str) -> (Arc<FakeBackend>, WindowsProxyIntegration, PathBuf) {
        let path = temp_journal(label);
        let _ = fs::remove_file(&path);
        let backend = Arc::new(FakeBackend {
            value: Mutex::new(ProxySettings {
                enabled: Some(0),
                server: Some("prior:80".to_owned()),
                bypass: None,
            }),
            writes: Mutex::new(Vec::new()),
            fail_notify: Mutex::new(0),
        });
        let integration = WindowsProxyIntegration::new(backend.clone(), &path);
        (backend, integration, path)
    }

    #[test]
    fn wildcard_listener_routes_windows_through_loopback_and_restores() {
        for (label, endpoint, expected) in [
            (
                "wildcard-v4",
                "0.0.0.0:8123",
                "http=127.0.0.1:8123;https=127.0.0.1:8123",
            ),
            (
                "wildcard-v6",
                "[::]:8123",
                "http=[::1]:8123;https=[::1]:8123",
            ),
        ] {
            let (backend, integration, path) = adapter(label);
            let prior = backend.read().unwrap();
            let token = integration.apply(endpoint.parse().unwrap()).unwrap();
            assert_eq!(backend.read().unwrap().server.as_deref(), Some(expected));
            integration.restore(&token).unwrap();
            assert_eq!(backend.read().unwrap(), prior);
            assert!(!path.exists());
        }
    }

    #[test]
    fn apply_and_retryable_restore_preserve_exact_state() {
        let (backend, integration, path) = adapter("restore");
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        assert!(path.exists());
        assert_eq!(backend.value.lock().unwrap().enabled, Some(1));
        integration.restore(&token).unwrap();
        integration.restore(&token).unwrap();
        assert_eq!(
            *backend.value.lock().unwrap(),
            ProxySettings {
                enabled: Some(0),
                server: Some("prior:80".to_owned()),
                bypass: None,
            }
        );
        assert!(!path.exists());
    }

    #[test]
    fn failed_apply_rolls_back_and_does_not_leave_dirty_journal() {
        let (backend, integration, path) = adapter("rollback");
        *backend.fail_notify.lock().unwrap() = 1;
        assert!(
            integration
                .apply("127.0.0.1:8124".parse().unwrap())
                .is_err()
        );
        assert_eq!(backend.value.lock().unwrap().enabled, Some(0));
        assert!(!path.exists());
    }

    #[test]
    fn crash_recovery_is_exact_and_idempotent() {
        let (backend, integration, path) = adapter("crash");
        let token = integration
            .apply("127.0.0.1:8125".parse().unwrap())
            .unwrap();
        drop(token); // A terminated process releases the lease without restoring.
        assert!(integration.recover_pending().unwrap());
        assert!(!integration.recover_pending().unwrap());
        assert_eq!(backend.value.lock().unwrap().enabled, Some(0));
        assert!(!path.exists());
    }

    #[derive(Default)]
    struct FakeTask {
        calls: Mutex<Vec<&'static str>>,
        fail_register: bool,
        fail_remove: bool,
    }

    impl ProxyRecoveryTask for FakeTask {
        fn register(&self) -> Result<(), WindowsHostError> {
            self.calls.lock().unwrap().push("register");
            if self.fail_register {
                Err(WindowsHostError::CommandFailed)
            } else {
                Ok(())
            }
        }

        fn remove(&self) -> Result<(), WindowsHostError> {
            self.calls.lock().unwrap().push("remove");
            if self.fail_remove {
                Err(WindowsHostError::CommandFailed)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn task_registration_and_cleanup_failures_do_not_block_proxy_use() {
        let (backend, integration, path) = adapter("task-unavailable");
        let prior = backend.read().unwrap();
        let task = Arc::new(FakeTask {
            fail_register: true,
            fail_remove: true,
            ..FakeTask::default()
        });
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let messages = Arc::clone(&diagnostics);
        let integration = integration
            .with_recovery_task(task.clone())
            .with_diagnostics(move |message| messages.lock().unwrap().push(message.to_owned()));
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        assert_eq!(backend.read().unwrap().enabled, Some(1));
        integration.restore(&token).unwrap();
        assert_eq!(backend.read().unwrap(), prior);
        assert!(!path.exists());
        assert_eq!(*task.calls.lock().unwrap(), ["register", "remove"]);
        assert_eq!(diagnostics.lock().unwrap().len(), 2);
    }

    #[test]
    fn registration_happens_after_journaling_and_before_proxy_mutation() {
        struct OrderedTask {
            backend: Arc<FakeBackend>,
            journal: PathBuf,
        }
        impl ProxyRecoveryTask for OrderedTask {
            fn register(&self) -> Result<(), WindowsHostError> {
                assert!(self.journal.is_file());
                assert!(self.backend.writes.lock().unwrap().is_empty());
                Ok(())
            }
            fn remove(&self) -> Result<(), WindowsHostError> {
                Ok(())
            }
        }
        let (backend, integration, path) = adapter("task-order");
        let integration = integration.with_recovery_task(Arc::new(OrderedTask {
            backend,
            journal: path,
        }));
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        integration.restore(&token).unwrap();
    }

    #[test]
    fn failed_restore_keeps_the_journal_and_task_until_retry() {
        let (backend, integration, path) = adapter("restore-retry");
        let task = Arc::new(FakeTask::default());
        let integration = integration.with_recovery_task(task.clone());
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        *backend.fail_notify.lock().unwrap() = 1;
        assert!(integration.restore(&token).is_err());
        assert!(path.exists());
        assert_eq!(*task.calls.lock().unwrap(), ["register"]);
        integration.restore(&token).unwrap();
        assert!(!path.exists());
        assert_eq!(*task.calls.lock().unwrap(), ["register", "remove"]);
    }

    #[test]
    fn failed_background_recovery_preserves_journal_and_task_for_retry() {
        let (backend, integration, path) = adapter("background-retry");
        let task = Arc::new(FakeTask::default());
        let integration = integration.with_recovery_task(task.clone());
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        drop(token);
        *backend.fail_notify.lock().unwrap() = 1;
        assert!(integration.recover_pending().is_err());
        assert!(path.exists());
        assert_eq!(*task.calls.lock().unwrap(), ["register"]);
        assert!(integration.recover_pending().unwrap());
        assert_eq!(*task.calls.lock().unwrap(), ["register", "remove"]);
    }

    #[test]
    fn failed_activation_and_rollback_leave_recoverable_state() {
        let (backend, integration, path) = adapter("rollback-retry");
        let task = Arc::new(FakeTask::default());
        let integration = integration.with_recovery_task(task.clone());
        *backend.fail_notify.lock().unwrap() = 2;
        assert!(
            integration
                .apply("127.0.0.1:8123".parse().unwrap())
                .is_err()
        );
        assert!(path.exists());
        assert_eq!(*task.calls.lock().unwrap(), ["register"]);
        assert!(integration.recover_pending().unwrap());
        assert_eq!(*task.calls.lock().unwrap(), ["register", "remove"]);
    }

    #[test]
    fn recovery_cannot_override_a_live_run_but_exit_can_restore_its_own_run() {
        let (backend, integration, path) = adapter("live-lease");
        let task = Arc::new(FakeTask::default());
        let integration = integration.with_recovery_task(task.clone());
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        let background =
            WindowsProxyIntegration::new(backend.clone(), &path).with_recovery_task(task.clone());
        assert!(matches!(
            background.recover_pending(),
            Err(WindowsHostError::ProxyInUse)
        ));
        assert!(background.apply("127.0.0.1:8124".parse().unwrap()).is_err());
        assert_eq!(backend.read().unwrap().enabled, Some(1));
        assert_eq!(*task.calls.lock().unwrap(), ["register"]);
        integration.recover_for_exit().unwrap();
        assert_eq!(backend.read().unwrap().enabled, Some(0));
        let next = background.apply("127.0.0.1:8124".parse().unwrap()).unwrap();
        integration.restore(&token).unwrap(); // An old token cannot undo a newer run.
        assert!(backend.read().unwrap().server.unwrap().contains("8124"));
        background.restore(&next).unwrap();
    }

    #[test]
    fn startup_and_maintenance_remove_stale_tasks_without_a_journal() {
        let (_, integration, _) = adapter("stale-task");
        let task = Arc::new(FakeTask::default());
        let integration = integration.with_recovery_task(task.clone());
        assert!(!integration.recover_pending().unwrap());
        assert_eq!(*task.calls.lock().unwrap(), ["remove"]);
    }

    #[test]
    fn different_journals_cannot_override_a_live_user_proxy() {
        let root = tempfile::tempdir().unwrap();
        let (backend, mut first, _) = adapter("shared-live");
        first.shared_owner = Some(SharedProxyOwner::new(root.path().to_owned()));
        let mut second =
            WindowsProxyIntegration::new(backend.clone(), root.path().join("other.json"));
        second.shared_owner.clone_from(&first.shared_owner);
        let token = first.apply("127.0.0.1:8123".parse().unwrap()).unwrap();
        assert!(matches!(
            second.recover_pending(),
            Err(WindowsHostError::ProxyInUse)
        ));
        assert!(second.apply("127.0.0.1:8124".parse().unwrap()).is_err());
        assert!(backend.read().unwrap().server.unwrap().contains("8123"));
        first.restore(&token).unwrap();
    }

    #[test]
    fn an_idle_adapter_can_exit_while_another_profile_owns_the_proxy() {
        let root = tempfile::tempdir().unwrap();
        let (backend, mut recording, journal) = adapter("idle-exit");
        recording.shared_owner = Some(SharedProxyOwner::new(root.path().to_owned()));
        let mut idle = WindowsProxyIntegration::new(backend.clone(), root.path().join("idle.json"));
        idle.shared_owner.clone_from(&recording.shared_owner);
        let token = recording.apply("127.0.0.1:8123".parse().unwrap()).unwrap();
        let during = backend.read().unwrap();
        assert!(!idle.recovery_pending());
        idle.recover_for_exit().unwrap();
        assert_eq!(backend.read().unwrap(), during);
        assert!(journal.exists());
        assert!(root.path().join("proxy-owner-v1.json").exists());
        assert!(matches!(
            recording.recover_pending(),
            Err(WindowsHostError::ProxyInUse)
        ));
        // An outstanding journal of our own still requires attention, even
        // while the other profile prevents us from recovering it safely.
        fs::write(&idle.journal_path, b"pending recovery").unwrap();
        assert!(matches!(
            idle.recover_for_exit(),
            Err(WindowsHostError::ProxyInUse)
        ));
        fs::remove_file(&idle.journal_path).unwrap();
        recording.restore(&token).unwrap();
    }

    #[test]
    fn another_profile_recovers_an_interrupted_owner_before_saving_its_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let (backend, mut first, path) = adapter("shared-crash");
        let prior = backend.read().unwrap();
        first.shared_owner = Some(SharedProxyOwner::new(root.path().to_owned()));
        let mut second =
            WindowsProxyIntegration::new(backend.clone(), root.path().join("other.json"));
        second.shared_owner.clone_from(&first.shared_owner);
        let token = first.apply("127.0.0.1:8123".parse().unwrap()).unwrap();
        drop(token);
        let next = second.apply("127.0.0.1:8124".parse().unwrap()).unwrap();
        assert!(!path.exists());
        second.restore(&next).unwrap();
        assert_eq!(backend.read().unwrap(), prior);
        assert!(!root.path().join("proxy-owner-v1.json").exists());
    }

    #[test]
    fn another_profiles_pending_recovery_is_visible_and_retryable() {
        let root = tempfile::tempdir().unwrap();
        let (backend, mut first, _) = adapter("shared-status");
        first.shared_owner = Some(SharedProxyOwner::new(root.path().to_owned()));
        let token = first.apply("127.0.0.1:8123".parse().unwrap()).unwrap();
        let mut second =
            WindowsProxyIntegration::new(backend.clone(), root.path().join("other.json"));
        second.shared_owner.clone_from(&first.shared_owner);
        assert!(!second.recovery_pending());
        drop(token);
        assert!(second.recovery_pending());
        assert!(second.recover_pending().unwrap());
        assert!(!second.recovery_pending());
        assert_eq!(backend.read().unwrap().enabled, Some(0));
    }

    #[cfg(windows)]
    #[test]
    fn recovery_through_a_directory_junction_clears_the_original_owner() {
        use std::os::windows::process::CommandExt as _;
        struct Tasks {
            journal: PathBuf,
            registered: Arc<Mutex<std::collections::BTreeSet<PathBuf>>>,
        }
        impl ProxyRecoveryTask for Tasks {
            fn register(&self) -> Result<(), WindowsHostError> {
                self.registered.lock().unwrap().insert(self.journal.clone());
                Ok(())
            }
            fn remove(&self) -> Result<(), WindowsHostError> {
                self.remove_for_journal(&self.journal)
            }
            fn remove_for_journal(&self, journal: &Path) -> Result<(), WindowsHostError> {
                self.registered.lock().unwrap().remove(journal);
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let alias = root.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        let result = Command::new("cmd.exe")
            .creation_flags(0x0800_0000)
            .args(["/c", "mklink", "/J"])
            .arg(&alias)
            .arg(&real)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(
            std::fs::canonicalize(&alias).unwrap(),
            std::fs::canonicalize(&real).unwrap()
        );
        let (backend, _, _) = adapter("junction");
        let prior = backend.read().unwrap();
        let registered = Arc::new(Mutex::new(std::collections::BTreeSet::new()));
        let owner = SharedProxyOwner::new(root.path().join("owner"));
        let mut first = WindowsProxyIntegration::new(backend.clone(), real.join("proxy.json"))
            .with_recovery_task(Arc::new(Tasks {
                journal: real.join("proxy.json"),
                registered: registered.clone(),
            }));
        first.shared_owner = Some(owner.clone());
        let token = first.apply("127.0.0.1:8123".parse().unwrap()).unwrap();
        drop(token);
        let mut recovery = WindowsProxyIntegration::new(backend.clone(), alias.join("proxy.json"))
            .with_recovery_task(Arc::new(Tasks {
                journal: alias.join("proxy.json"),
                registered: registered.clone(),
            }));
        recovery.shared_owner = Some(owner);
        assert!(recovery.recover_pending().unwrap());
        assert!(!recovery.recovery_pending());
        assert!(registered.lock().unwrap().is_empty());
        assert!(!root.path().join("owner/proxy-owner-v1.json").exists());
        assert_eq!(backend.read().unwrap(), prior);

        // Also cover termination after the snapshot was consumed but before
        // the owner record and original task could be removed.
        let token = first.apply("127.0.0.1:8123".parse().unwrap()).unwrap();
        drop(token);
        backend.write(&prior).unwrap();
        fs::remove_file(real.join("proxy.json")).unwrap();
        assert!(!recovery.recover_pending().unwrap());
        assert!(registered.lock().unwrap().is_empty());
        assert!(!recovery.recovery_pending());
    }

    #[test]
    fn recovery_preserves_a_subsequently_selected_vpn_or_proxy() {
        let (backend, integration, path) = adapter("foreign-proxy");
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        let vpn = ProxySettings {
            enabled: Some(1),
            server: Some("vpn.example:8080".into()),
            bypass: Some("vpn.local".into()),
        };
        backend.write(&vpn).unwrap();
        drop(token);
        assert!(integration.recover_pending().unwrap());
        assert_eq!(backend.read().unwrap(), vpn);
        assert!(!path.exists());
    }

    #[test]
    fn normal_stop_preserves_later_bypass_and_enable_changes() {
        let (backend, integration, _) = adapter("later-bypass");
        let prior = backend.read().unwrap();
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        let mut later = backend.read().unwrap();
        later.enabled = None;
        later.bypass = Some("user-added.example".into());
        backend.write(&later).unwrap();
        integration.restore(&token).unwrap();
        let restored = backend.read().unwrap();
        assert_eq!(restored.enabled, None);
        assert_eq!(restored.bypass, later.bypass);
        assert_eq!(restored.server, prior.server);
    }

    #[test]
    fn reselecting_the_previous_endpoint_keeps_its_new_enabled_state() {
        let (backend, integration, _) = adapter("reselected-proxy");
        let prior = backend.read().unwrap();
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        let selected = ProxySettings {
            enabled: Some(1),
            server: prior.server,
            bypass: prior.bypass,
        };
        backend.write(&selected).unwrap();
        integration.restore(&token).unwrap();
        assert_eq!(backend.read().unwrap(), selected);
    }

    #[test]
    fn recovery_completes_a_restore_interrupted_before_the_bypass_write() {
        let (backend, integration, _) = adapter("partial-restore");
        let prior = backend.read().unwrap();
        let token = integration
            .apply("127.0.0.1:8123".parse().unwrap())
            .unwrap();
        let mut partial = prior.clone();
        partial.bypass = Some("<local>".into());
        backend.write(&partial).unwrap();
        drop(token);
        assert!(integration.recover_pending().unwrap());
        assert_eq!(backend.read().unwrap(), prior);
    }

    #[test]
    fn recovery_rolls_back_an_apply_interrupted_after_enabling_the_old_endpoint() {
        let (backend, integration, path) = adapter("partial-apply");
        let prior = backend.read().unwrap();
        let applied = ProxySettings {
            enabled: Some(1),
            server: Some("http=127.0.0.1:8123;https=127.0.0.1:8123".into()),
            bypass: Some("<local>".into()),
        };
        let journal = write_journal(&path, &prior, &applied).unwrap();
        assert!(!journal.application_complete);
        let mut partial = prior.clone();
        partial.enabled = applied.enabled;
        backend.write(&partial).unwrap();
        assert!(integration.recover_pending().unwrap());
        assert_eq!(backend.read().unwrap(), prior);
    }

    #[cfg(windows)]
    #[test]
    fn powershell_restore_decisions_match_the_model_without_registry_writes() {
        let prior = ProxySettings {
            enabled: Some(0),
            server: Some("prior:80".into()),
            bypass: None,
        };
        let applied = ProxySettings {
            enabled: Some(1),
            server: Some("http=127.0.0.1:8123;https=127.0.0.1:8123".into()),
            bypass: Some("<local>".into()),
        };
        let currents = [
            applied.clone(),
            prior.clone(),
            ProxySettings {
                enabled: Some(1),
                server: prior.server.clone(),
                bypass: applied.bypass.clone(),
            },
            ProxySettings {
                bypass: applied.bypass.clone(),
                ..prior.clone()
            },
            ProxySettings {
                bypass: Some("user.example".into()),
                ..applied.clone()
            },
            ProxySettings {
                enabled: None,
                ..applied.clone()
            },
            ProxySettings {
                server: Some("vpn:90".into()),
                ..applied.clone()
            },
        ];
        let cases: Vec<_> = currents.iter().flat_map(|current| [false, true].map(|complete| serde_json::json!({ "prior": prior, "applied": applied, "current": current, "applicationComplete": complete }))).collect();
        let script = format!(
            "$cases = $env:TRANSMOG_INPUT | ConvertFrom-Json\nforeach ($snapshot in $cases) {{ $current = $snapshot.current\n{RESTORE_PLAN_SCRIPT}\n$restore | ConvertTo-Json -Compress\n}}"
        );
        let output = run_script_bounded(
            &script,
            Some(&serde_json::to_string(&cases).unwrap()),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        let actual: Vec<ProxySettings> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let expected: Vec<_> = currents
            .iter()
            .flat_map(|current| {
                [false, true]
                    .map(|complete| restored_settings(&prior, Some(&applied), current, complete))
            })
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn journal_publication_never_replaces_an_existing_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("snapshot.json");
        std::fs::write(&path, b"existing snapshot").unwrap();
        assert!(
            write_journal(&path, &ProxySettings::default(), &ProxySettings::default()).is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"existing snapshot");
    }

    #[test]
    fn an_invalid_snapshot_path_does_not_disarm_recovery() {
        let (_, integration, path) = adapter("invalid-snapshot-path");
        std::fs::create_dir(&path).unwrap();
        let task = Arc::new(FakeTask::default());
        let integration = integration.with_recovery_task(task.clone());
        assert!(integration.recovery_pending());
        assert!(integration.recover_pending().is_err());
        assert!(task.calls.lock().unwrap().is_empty());
        std::fs::remove_dir(path).unwrap();
    }

    #[test]
    fn an_unrecoverably_large_snapshot_cannot_change_proxy_settings() {
        let (backend, integration, path) = adapter("large-snapshot");
        backend.value.lock().unwrap().bypass = Some("a".repeat(64 * 1024));
        assert!(
            integration
                .apply("127.0.0.1:8123".parse().unwrap())
                .is_err()
        );
        assert!(backend.writes.lock().unwrap().is_empty());
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_stuck_task_scheduler_command_has_a_bounded_lifetime() {
        let started = std::time::Instant::now();
        let result = run_script_bounded(
            "Start-Sleep -Seconds 30",
            None,
            std::time::Duration::from_millis(100),
        );
        assert!(matches!(result, Err(WindowsHostError::CommandTimedOut)));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn rejects_non_loopback_and_ambiguous_thumbprints() {
        let (_, integration, _) = adapter("reject");
        assert!(
            integration
                .apply("192.0.2.1:8126".parse().unwrap())
                .is_err()
        );
        assert!(matches!(
            validate_thumbprint("ABCD"),
            Err(WindowsHostError::InvalidThumbprint)
        ));
    }

    #[test]
    fn certificate_ownership_is_exact_idempotent_and_conflict_safe() {
        let path = temp_journal("certificate-ownership");
        let _ = fs::remove_file(&path);
        let registry = OwnedCertificateRegistry::new(&path);
        let first = "11".repeat(32);
        let other = "22".repeat(32);
        assert!(registry.claim(&first).unwrap());
        assert!(!registry.claim(&first).unwrap());
        assert_eq!(registry.owned_thumbprint().unwrap(), Some(first.clone()));
        assert!(matches!(
            registry.claim(&other),
            Err(WindowsHostError::CertificateOwnershipConflict)
        ));
        assert!(matches!(
            registry.clear(&other),
            Err(WindowsHostError::CertificateOwnershipConflict)
        ));
        registry.clear(&first).unwrap();
        assert_eq!(registry.owned_thumbprint().unwrap(), None);
    }

    #[cfg(windows)]
    #[test]
    fn private_key_acl_can_be_applied_to_an_owned_test_file() {
        let path = std::env::temp_dir().join(format!(
            "transmog-key-protection-{}.key",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        fs::write(&path, b"test-only-key-material").unwrap();
        CurrentUserKeyProtection.protect(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"test-only-key-material");
        fs::remove_file(path).unwrap();
    }
}
