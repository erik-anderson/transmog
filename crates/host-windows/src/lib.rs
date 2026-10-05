#![deny(missing_docs)]

//! Transactional current-user Windows proxy and certificate integration.
//!
//! System-proxy changes are protected by an atomically created recovery
//! journal. Certificate trust is deliberately a separate, explicit workflow.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use transmog_session::{HostIntegration, HostIntegrationError, HostRestoreToken};

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryJournal {
    schema: u32,
    prior: ProxySettings,
}

#[derive(Clone, Debug)]
struct RestoreState {
    prior: ProxySettings,
}

/// Transactional current-user system-proxy integration.
#[derive(Clone)]
pub struct WindowsProxyIntegration {
    backend: Arc<dyn ProxySettingsBackend>,
    journal_path: PathBuf,
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
        Self::new(Arc::new(SystemProxyBackend), journal_path)
    }

    /// Creates an adapter around an injected backend.
    pub fn new(backend: Arc<dyn ProxySettingsBackend>, journal_path: impl Into<PathBuf>) -> Self {
        Self {
            backend,
            journal_path: journal_path.into(),
        }
    }

    /// Returns whether a prior process left a recovery journal.
    pub fn recovery_pending(&self) -> bool {
        self.journal_path.is_file()
    }

    /// Restores a crash journal before a new proxy run.
    ///
    /// # Errors
    ///
    /// Returns a bounded I/O, journal, registry, or notification failure. A
    /// failed recovery preserves the journal so retrying is safe.
    pub fn recover_pending(&self) -> Result<bool, WindowsHostError> {
        if !self.recovery_pending() {
            return Ok(false);
        }
        let bytes = fs::read(&self.journal_path).map_err(WindowsHostError::Io)?;
        if bytes.len() > 64 * 1024 {
            return Err(WindowsHostError::InvalidJournal);
        }
        let journal: RecoveryJournal =
            serde_json::from_slice(&bytes).map_err(WindowsHostError::Journal)?;
        if journal.schema != 1 {
            return Err(WindowsHostError::InvalidJournal);
        }
        self.restore_settings(&journal.prior)?;
        remove_journal(&self.journal_path)?;
        Ok(true)
    }

    fn restore_settings(&self, settings: &ProxySettings) -> Result<(), WindowsHostError> {
        self.backend.write(settings)?;
        self.backend.notify_changed()
    }
}

impl HostIntegration for WindowsProxyIntegration {
    fn apply(&self, endpoint: SocketAddr) -> Result<HostRestoreToken, HostIntegrationError> {
        if !endpoint.ip().is_loopback() {
            return Err(host_error("system proxy endpoint must be loopback"));
        }
        if self.recovery_pending() {
            return Err(host_error("a Windows proxy recovery journal is pending"));
        }
        let prior = self.backend.read().map_err(|error| to_host_error(&error))?;
        write_journal(&self.journal_path, &prior).map_err(|error| to_host_error(&error))?;
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
        if let Err(error) = self
            .backend
            .write(&desired)
            .and_then(|()| self.backend.notify_changed())
        {
            let _ = self.restore_settings(&prior);
            let _ = remove_journal(&self.journal_path);
            return Err(to_host_error(&error));
        }
        Ok(HostRestoreToken::new(RestoreState { prior }))
    }

    fn restore(&self, token: &HostRestoreToken) -> Result<(), HostIntegrationError> {
        let state = token
            .downcast_ref::<RestoreState>()
            .ok_or_else(|| host_error("Windows proxy restore token type mismatch"))?;
        self.restore_settings(&state.prior)
            .map_err(|error| to_host_error(&error))?;
        remove_journal(&self.journal_path).map_err(|error| to_host_error(&error))
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

/// Durable exact-thumbprint ownership record for an app-installed current-user
/// root certificate.
#[derive(Clone, Debug)]
pub struct OwnedCertificateRegistry {
    path: PathBuf,
}

impl OwnedCertificateRegistry {
    /// Creates a registry at an application-owned path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Records the exact certificate before attempting trust-store mutation.
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

fn write_journal(path: &Path, prior: &ProxySettings) -> Result<(), WindowsHostError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(WindowsHostError::Io)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(WindowsHostError::Io)?;
    let bytes = serde_json::to_vec(&RecoveryJournal {
        schema: 1,
        prior: prior.clone(),
    })
    .map_err(WindowsHostError::Journal)?;
    file.write_all(&bytes).map_err(WindowsHostError::Io)?;
    file.sync_all().map_err(WindowsHostError::Io)
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

fn run_script(script: &str, input: Option<&str>) -> Result<String, WindowsHostError> {
    if !cfg!(windows) {
        return Err(WindowsHostError::UnsupportedPlatform);
    }
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    if let Some(input) = input {
        command.env("TRANSMOG_INPUT", input);
    }
    let output = command.output().map_err(WindowsHostError::Io)?;
    if !output.status.success() {
        return Err(WindowsHostError::CommandFailed);
    }
    String::from_utf8(output.stdout).map_err(|_| WindowsHostError::CommandFailed)
}

fn to_host_error(error: &WindowsHostError) -> HostIntegrationError {
    host_error(error.to_string())
}

fn host_error(message: impl Into<String>) -> HostIntegrationError {
    HostIntegrationError::new(message.into())
}

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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug)]
    struct FakeBackend {
        value: Mutex<ProxySettings>,
        writes: Mutex<Vec<ProxySettings>>,
        fail_notify: Mutex<bool>,
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
            if *self.fail_notify.lock().unwrap() {
                *self.fail_notify.lock().unwrap() = false;
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
            fail_notify: Mutex::new(false),
        });
        let integration = WindowsProxyIntegration::new(backend.clone(), &path);
        (backend, integration, path)
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
        *backend.fail_notify.lock().unwrap() = true;
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
        let _token = integration
            .apply("127.0.0.1:8125".parse().unwrap())
            .unwrap();
        assert!(integration.recover_pending().unwrap());
        assert!(!integration.recover_pending().unwrap());
        assert_eq!(backend.value.lock().unwrap().enabled, Some(0));
        assert!(!path.exists());
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
