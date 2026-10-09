//! CLI-owned roots. Each durable identity is independent so a canceled cleanup
//! never prevents recording a fresh ephemeral root. Ephemeral keys stay in CA memory.
use crate::root_lifecycle::{KeyStorage, Lifecycle, RootLifecycle, millis};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use transmog_tls::ProxyCa;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RootRecord {
    schema: u32,
    pub(crate) sha256: String,
    pub(crate) persistent: bool,
    #[serde(default)]
    pub(crate) lifecycle: Lifecycle,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrivacyRecord {
    schema: u32,
    generation: u64,
    redact_sensitive_headers: bool,
    #[serde(default = "default_request_body_limit")]
    request_body_limit: Option<u64>,
}

#[allow(clippy::unnecessary_wraps)] // Serde needs the optional field type; null means Unlimited.
const fn default_request_body_limit() -> Option<u64> {
    Some(transmog_capture::DEFAULT_REQUEST_BODY_CAPTURE_BYTES)
}

pub(crate) trait RootTrust {
    fn install(&self, certificate: &Path, sha256: &str) -> io::Result<()>;
    /// Success means the exact owned root is absent, including after an earlier removal.
    fn remove(&self, certificate: &Path, sha256: &str) -> io::Result<()>;
}

pub(crate) struct RootLedger {
    directory: PathBuf,
    _lock: File,
}
impl RootLedger {
    pub(crate) fn open(directory: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        protect_directory(&directory)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let lock = options
            .open(directory.join("root-workflow.lock"))
            .map_err(|_| {
                io::Error::other(
                    "Another CLI root workflow is active, or its state directory is unreadable",
                )
            })?;
        #[cfg(unix)]
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
    #[cfg(test)]
    pub(crate) fn redaction(&self, choice: Option<bool>) -> io::Result<bool> {
        self.preferences(choice, None).map(|(redact, _)| redact)
    }

    #[allow(clippy::option_option)] // None preserves the preference; Some(None) selects Unlimited.
    pub(crate) fn preferences(
        &self,
        redaction: Option<bool>,
        body_limit: Option<Option<u64>>,
    ) -> io::Result<(bool, Option<u64>)> {
        if body_limit == Some(Some(0)) {
            return Err(io::Error::other(
                "Request body limit must be positive or Unlimited",
            ));
        }
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("privacy-")
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                if paths.len() >= 16 {
                    return Err(io::Error::other("Too many CLI privacy generations"));
                }
                paths.push(entry.path());
            }
        }
        paths.sort();
        let mut latest = None;
        let mut previous_path = None;
        for path in paths.iter().rev() {
            if let Ok(bytes) = read_bounded(path, 8192)
                && let Ok(record) = serde_json::from_slice::<PrivacyRecord>(&bytes)
                && record.schema == 1
                && record.request_body_limit != Some(0)
                && path.file_name().is_some_and(|name| {
                    name == format!("privacy-{:020}.json", record.generation).as_str()
                })
            {
                latest = Some(record);
                previous_path = Some(path);
                break;
            }
        }
        if latest.is_none() && !paths.is_empty() {
            return Err(io::Error::other(
                "CLI privacy preferences are unreadable; restore them before recording",
            ));
        }
        let previous = latest
            .as_ref()
            .is_some_and(|record| record.redact_sensitive_headers);
        let previous_limit = latest
            .as_ref()
            .map_or(default_request_body_limit(), |record| {
                record.request_body_limit
            });
        let redact = redaction.unwrap_or(previous);
        let limit = body_limit.unwrap_or(previous_limit);
        if redaction.is_none() && body_limit.is_none()
            || latest.is_some() && redact == previous && limit == previous_limit
        {
            return Ok((redact, limit));
        }
        let generation = paths
            .iter()
            .filter_map(|path| {
                path.file_stem()?
                    .to_str()?
                    .strip_prefix("privacy-")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| io::Error::other("CLI privacy generation is exhausted"))?;
        let record = PrivacyRecord {
            schema: 1,
            generation,
            redact_sensitive_headers: redact,
            request_body_limit: limit,
        };
        write_new(
            &self
                .directory
                .join(format!("privacy-{generation:020}.json")),
            &serde_json::to_vec(&record).map_err(io::Error::other)?,
        )?;
        for path in &paths {
            if Some(path) != previous_path {
                remove_if_present(path)?;
            }
        }
        Ok((redact, limit))
    }
    fn path(&self, hash: &str, extension: &str) -> PathBuf {
        self.directory.join(format!("root-{hash}.{extension}"))
    }
    pub(crate) fn certificate(&self, record: &RootRecord) -> PathBuf {
        self.path(&record.sha256, "pem")
    }
    pub(crate) fn records(&self) -> io::Result<Vec<RootRecord>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("root-")
                || !entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            if records.len() >= 128 {
                return Err(io::Error::other(
                    "Too many pending CLI roots; run transmog-cli roots cleanup",
                ));
            }
            let bytes = read_bounded(&entry.path(), 8192)?;
            let record: RootRecord = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if !matches!(record.schema, 1 | 2)
                || !record.lifecycle.valid()
                || !valid_hash(&record.sha256)
                || name != format!("root-{}.json", record.sha256)
            {
                return Err(io::Error::other(
                    "Invalid CLI root ownership record; no trust changes were made",
                ));
            }
            records.push(record);
        }
        records.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        Ok(records)
    }
    pub(crate) fn prepare(&self, persistent: bool) -> io::Result<(ProxyCa, RootRecord)> {
        let records = self.records()?;
        if persistent {
            for record in records
                .iter()
                .filter(|record| record.persistent && !record.lifecycle.retired)
            {
                let ca = (|| {
                    let ca = ProxyCa::from_pem(
                        &read_bounded(&self.certificate(record), 8192)?,
                        &read_bounded(&self.path(&record.sha256, "key"), 8192)?,
                    )
                    .map_err(io::Error::other)?;
                    if ca.sha256_thumbprint().map_err(io::Error::other)? != record.sha256 {
                        return Err(io::Error::other("Persistent root identity mismatch"));
                    }
                    if !crate::root_lifecycle::reusable(&ca)? {
                        return Err(io::Error::other(
                            "Persistent root expires within seven days or is not valid yet",
                        ));
                    }
                    Ok(ca)
                })();
                match ca {
                    Ok(ca) => {
                        let updated =
                            self.update(record, |updated| updated.lifecycle.begin(&ca, true))?;
                        println!("Reusing persistent CLI root {}.", record.sha256);
                        return Ok((ca, updated));
                    }
                    Err(error) => {
                        self.update(record, |updated| {
                            updated.lifecycle.retired = true;
                            updated.lifecycle.state = RootLifecycle::Retired;
                            updated.lifecycle.last_cleanup_error =
                                Some(error.to_string().chars().take(512).collect());
                            Ok(())
                        })?;
                        remove_if_present(&self.path(&record.sha256, "key"))?;
                        self.update(record, |updated| {
                            updated.lifecycle.key_storage = KeyStorage::Removed;
                            Ok(())
                        })?;
                        println!(
                            "Rotating persistent CLI root {}: {error}. Its public identity remains for cleanup.",
                            record.sha256
                        );
                    }
                }
            }
        }
        if records.len() >= 128 {
            return Err(io::Error::other(
                "Clean up pending CLI roots before creating another",
            ));
        }
        let ca = ProxyCa::generate(
            "Transmog CLI support capture root",
            if persistent { 365 } else { 2 },
        )
        .map_err(io::Error::other)?;
        self.save_new_ca(ca, persistent)
    }
    fn save_new_ca(&self, ca: ProxyCa, persistent: bool) -> io::Result<(ProxyCa, RootRecord)> {
        let mut lifecycle = Lifecycle::default();
        lifecycle.begin(&ca, persistent)?;
        let record = RootRecord {
            schema: 2,
            lifecycle,
            sha256: ca.sha256_thumbprint().map_err(io::Error::other)?,
            persistent,
        };
        // Publish ownership before any possible OS trust-store change. Never export
        // or serialize the ephemeral private key, even to a temporary file.
        write_new(
            &self.path(&record.sha256, "json"),
            &serde_json::to_vec(&record).map_err(io::Error::other)?,
        )?;
        write_new(
            &self.certificate(&record),
            &ca.certificate_pem().map_err(io::Error::other)?,
        )?;
        if persistent {
            write_new(
                &self.path(&record.sha256, "key"),
                &ca.private_key_pem_pkcs8().map_err(io::Error::other)?,
            )?;
        }
        Ok((ca, record))
    }
    fn update(
        &self,
        record: &RootRecord,
        change: impl FnOnce(&mut RootRecord) -> io::Result<()>,
    ) -> io::Result<RootRecord> {
        let path = self.path(&record.sha256, "json");
        let mut latest: RootRecord =
            serde_json::from_slice(&read_bounded(&path, 8192)?).map_err(io::Error::other)?;
        if latest.sha256 != record.sha256
            || latest.persistent != record.persistent
            || !matches!(latest.schema, 1 | 2)
            || !latest.lifecycle.valid()
        {
            return Err(io::Error::other("CLI root ownership changed"));
        }
        change(&mut latest)?;
        latest.schema = 2;
        if !latest.lifecycle.valid() {
            return Err(io::Error::other("Invalid CLI root lifecycle"));
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.write_all(&serde_json::to_vec(&latest).map_err(io::Error::other)?)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(latest)
    }
    pub(crate) fn mark(&self, record: &RootRecord, state: RootLifecycle) -> io::Result<()> {
        self.update(record, |updated| {
            updated.lifecycle.state = state;
            if state == RootLifecycle::Installed {
                updated.lifecycle.installed_at = Some(millis());
            }
            Ok(())
        })
        .map(|_| ())
    }
    pub(crate) fn finish_run(&self, record: &RootRecord, success: bool) -> io::Result<()> {
        self.update(record, |updated| {
            updated.lifecycle.state = if updated.persistent {
                RootLifecycle::PersistentIdle
            } else {
                RootLifecycle::CaptureFinished
            };
            updated.lifecycle.finished_at = Some(millis());
            updated.lifecycle.outcome = Some(
                if success {
                    "capture-sealed"
                } else {
                    "capture-failed-or-setup-incomplete"
                }
                .into(),
            );
            Ok(())
        })
        .map(|_| ())
    }
    pub(crate) fn cleanup(&self, record: &RootRecord, trust: &dyn RootTrust) -> io::Result<()> {
        // Persist intent before either OS consent or key deletion. A failed/canceled
        // prompt preserves the public identity; the next run does not reuse this key.
        self.update(record, |updated| {
            updated.lifecycle.retired = true;
            updated.lifecycle.state = RootLifecycle::CleanupPending;
            updated.lifecycle.cleanup_attempts =
                updated.lifecycle.cleanup_attempts.saturating_add(1);
            updated.lifecycle.last_cleanup_at = Some(millis());
            Ok(())
        })?;
        let result = (|| {
            remove_if_present(&self.path(&record.sha256, "key"))?;
            self.update(record, |updated| {
                updated.lifecycle.key_storage = KeyStorage::Removed;
                Ok(())
            })?;
            trust.remove(&self.certificate(record), &record.sha256)?;
            remove_if_present(&self.certificate(record))?;
            remove_if_present(&self.path(&record.sha256, "json"))
        })();
        if let Err(error) = &result {
            self.update(record, |updated| {
                updated.lifecycle.last_cleanup_error =
                    Some(error.to_string().chars().take(512).collect());
                Ok(())
            })?;
        }
        result
    }
    pub(crate) fn pending_cleanup(&self, record: &RootRecord) -> bool {
        !record.persistent
            || record.lifecycle.retired
            || !self.path(&record.sha256, "key").is_file()
    }
    pub(crate) fn cleanup_ephemeral(&self, trust: &dyn RootTrust) -> io::Result<usize> {
        self.cleanup_pending(trust, None)
    }
    pub(crate) fn cleanup_pending(
        &self,
        trust: &dyn RootTrust,
        exclude: Option<&str>,
    ) -> io::Result<usize> {
        let mut pending = 0;
        for record in self
            .records()?
            .iter()
            .filter(|record| self.pending_cleanup(record))
        {
            if exclude == Some(record.sha256.as_str()) {
                continue;
            }
            if let Err(error) = self.cleanup(record, trust) {
                eprintln!("Root {} still needs cleanup: {error}", record.sha256);
                pending += 1;
            }
        }
        Ok(pending)
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase())
}
fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other(
            "CLI root files must not be symbolic links",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other(
            "CLI root record or material exceeds its size limit",
        ));
    }
    Ok(bytes)
}
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Root file has no parent"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
fn protect_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    transmog_host_windows::CurrentUserKeyProtection
        .protect_directory(path)
        .map_err(io::Error::other)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(crate) fn state_directory() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let directory =
        env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("Transmog-cli"));
    #[cfg(target_os = "macos")]
    let directory = env::var_os("HOME")
        .map(|base| PathBuf::from(base).join("Library/Application Support/Transmog-cli"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let directory = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|base| PathBuf::from(base).join(".local/state")))
        .map(|base| base.join("Transmog-cli"));
    directory.ok_or_else(|| io::Error::other("The current-user CLI state directory is unavailable"))
}

pub(crate) struct SystemRootTrust;
impl RootTrust for SystemRootTrust {
    fn install(&self, certificate: &Path, sha256: &str) -> io::Result<()> {
        #[cfg(windows)]
        {
            let store = transmog_host_windows::CurrentUserCertificateStore;
            if !store.contains(sha256).map_err(io::Error::other)? {
                store
                    .install(certificate, sha256)
                    .map_err(io::Error::other)?;
            }
            if !store.contains(sha256).map_err(io::Error::other)? {
                return Err(io::Error::other(
                    "Windows did not trust the root; approve its certificate prompt and try again",
                ));
            }
        }
        #[cfg(target_os = "macos")]
        {
            verify_public(certificate, sha256)?;
            checked(
                std::process::Command::new("/usr/bin/security")
                    .args(["add-trusted-cert", "-r", "trustRoot", "-p", "ssl", "-k"])
                    .arg(login_keychain()?)
                    .arg(certificate),
            )?;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            verify_public(certificate, sha256)?;
            let (target, update) = linux_root(sha256)?;
            if target.exists() {
                verify_public(&target, sha256)?;
            } else {
                checked(
                    std::process::Command::new("sudo")
                        .args(["install", "-m", "644", "--"])
                        .arg(certificate)
                        .arg(&target),
                )?;
            }
            checked(std::process::Command::new("sudo").args(update))?;
        }
        Ok(())
    }
    fn remove(&self, certificate: &Path, sha256: &str) -> io::Result<()> {
        #[cfg(windows)]
        {
            let _ = certificate;
            let store = transmog_host_windows::CurrentUserCertificateStore;
            if store.contains(sha256).map_err(io::Error::other)? {
                store.remove(sha256).map_err(io::Error::other)?;
            }
            if store.contains(sha256).map_err(io::Error::other)? {
                return Err(io::Error::other(
                    "The root is still trusted; approve the removal prompt",
                ));
            }
        }
        #[cfg(target_os = "macos")]
        {
            let _ = certificate;
            let keychain = login_keychain()?;
            let output = std::process::Command::new("/usr/bin/security")
                .args(["find-certificate", "-a", "-Z"])
                .arg(&keychain)
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other(
                    "Could not inspect the login keychain for cleanup",
                ));
            }
            if String::from_utf8_lossy(&output.stdout).contains(sha256) {
                checked(
                    std::process::Command::new("/usr/bin/security")
                        .args(["delete-certificate", "-t", "-Z", sha256])
                        .arg(&keychain),
                )?;
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let (target, update) = linux_root(sha256)?;
            let pending = certificate.with_extension("refresh");
            if target.exists() {
                verify_public(&target, sha256)?;
                if !pending.exists() {
                    write_new(&pending, b"certificate bundle refresh pending")?;
                }
                checked(
                    std::process::Command::new("sudo")
                        .args(["rm", "--"])
                        .arg(target),
                )?;
            }
            // Retry rebuilding even if a previous removal deleted the anchor
            // but was canceled before the OS bundle update completed.
            if pending.exists() {
                checked(std::process::Command::new("sudo").args(update))?;
                remove_if_present(&pending)?;
            }
        }
        Ok(())
    }
}
#[cfg(unix)]
fn verify_public(path: &Path, hash: &str) -> io::Result<()> {
    use boring::{hash::MessageDigest, x509::X509};
    let certificate = X509::from_pem(&read_bounded(path, 8192)?).map_err(io::Error::other)?;
    let actual = certificate
        .digest(MessageDigest::sha256())
        .map_err(io::Error::other)?
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    if actual == hash {
        Ok(())
    } else {
        Err(io::Error::other(
            "Certificate identity changed; cleanup was refused",
        ))
    }
}
#[cfg(unix)]
fn checked(command: &mut std::process::Command) -> io::Result<()> {
    if command.status()?.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            "OS certificate action was canceled or failed; ownership metadata has been retained",
        ))
    }
}
#[cfg(target_os = "macos")]
fn login_keychain() -> io::Result<PathBuf> {
    env::var_os("HOME")
        .map(|base| PathBuf::from(base).join("Library/Keychains/login.keychain-db"))
        .ok_or_else(|| io::Error::other("Login keychain path is unavailable"))
}
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_root(hash: &str) -> io::Result<(PathBuf, Vec<&'static str>)> {
    if Path::new("/usr/local/share/ca-certificates").is_dir() {
        Ok((
            PathBuf::from(format!(
                "/usr/local/share/ca-certificates/transmog-cli-{hash}.crt"
            )),
            vec!["update-ca-certificates"],
        ))
    } else if Path::new("/etc/pki/ca-trust/source/anchors").is_dir() {
        Ok((
            PathBuf::from(format!(
                "/etc/pki/ca-trust/source/anchors/transmog-cli-{hash}.crt"
            )),
            vec!["update-ca-trust", "extract"],
        ))
    } else {
        Err(io::Error::other(
            "This Linux trust store is unsupported; use --no-install-root and install the public root for your application manually",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FakeTrust {
        fail: bool,
    }
    impl RootTrust for FakeTrust {
        fn install(&self, _: &Path, _: &str) -> io::Result<()> {
            Ok(())
        }
        fn remove(&self, _: &Path, _: &str) -> io::Result<()> {
            if self.fail {
                Err(io::Error::other("User canceled"))
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn ephemeral_keys_never_reach_disk_and_canceled_cleanup_keeps_multiple_identities() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        let (_, first) = ledger.prepare(false).unwrap();
        assert!(!ledger.path(&first.sha256, "key").exists());
        assert!(ledger.cleanup(&first, &FakeTrust { fail: true }).is_err());
        let (_, second) = ledger.prepare(false).unwrap();
        assert_ne!(first.sha256, second.sha256);
        assert_eq!(ledger.records().unwrap().len(), 2);
        drop(ledger);
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        assert_eq!(
            ledger
                .cleanup_ephemeral(&FakeTrust { fail: false })
                .unwrap(),
            0
        );
        assert!(ledger.records().unwrap().is_empty());
    }
    #[test]
    fn cleanup_recovery_keeps_lifecycle_without_ephemeral_keys() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        let (_, root) = ledger.prepare(false).unwrap();
        assert_eq!(root.lifecycle.key_storage, KeyStorage::MemoryOnly);
        assert!(root.lifecycle.expires_at.unwrap() > root.lifecycle.created_at.unwrap());
        ledger
            .mark(&root, RootLifecycle::InstallationRequested)
            .unwrap();
        ledger.mark(&root, RootLifecycle::Installed).unwrap();
        ledger.finish_run(&root, false).unwrap();
        assert!(ledger.cleanup(&root, &FakeTrust { fail: true }).is_err());
        let pending = &ledger.records().unwrap()[0];
        assert_eq!(pending.lifecycle.state, RootLifecycle::CleanupPending);
        assert_eq!(pending.lifecycle.key_storage, KeyStorage::Removed);
        assert_eq!(pending.lifecycle.cleanup_attempts, 1);
        assert!(pending.lifecycle.installed_at.is_some());
        assert!(pending.lifecycle.finished_at.is_some());
        assert_eq!(
            pending.lifecycle.outcome.as_deref(),
            Some("capture-failed-or-setup-incomplete")
        );
        assert!(
            pending
                .lifecycle
                .last_cleanup_error
                .as_deref()
                .unwrap()
                .contains("canceled")
        );
        drop(ledger);
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        let (_, next) = ledger.prepare(false).unwrap();
        assert_ne!(next.sha256, root.sha256);
        assert_ne!(next.lifecycle.run_id, root.lifecycle.run_id);
        ledger
            .cleanup_pending(&FakeTrust { fail: false }, Some(&next.sha256))
            .unwrap();
        assert_eq!(ledger.records().unwrap().len(), 1);
        assert!(!ledger.path(&next.sha256, "key").exists());
    }
    #[test]
    fn persistent_near_expiry_rotates_and_canceled_old_removal_does_not_block_reuse() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        let (_, old) = ledger
            .save_new_ca(ProxyCa::generate("Expiry fixture", 2).unwrap(), true)
            .unwrap();
        let (_, fresh) = ledger.prepare(true).unwrap();
        assert_ne!(old.sha256, fresh.sha256);
        assert!(!ledger.path(&old.sha256, "key").exists());
        assert!(
            ledger
                .records()
                .unwrap()
                .iter()
                .find(|record| record.sha256 == old.sha256)
                .unwrap()
                .lifecycle
                .retired
        );
        assert_eq!(
            ledger
                .cleanup_pending(&FakeTrust { fail: true }, Some(&fresh.sha256))
                .unwrap(),
            1
        );
        assert_eq!(ledger.prepare(true).unwrap().1.sha256, fresh.sha256);
        ledger
            .cleanup_pending(&FakeTrust { fail: false }, Some(&fresh.sha256))
            .unwrap();
        assert_eq!(ledger.records().unwrap().len(), 1);
    }
    #[test]
    fn legacy_owned_root_is_migrated_on_reuse_and_unknown_schema_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        let (_, root) = ledger.prepare(true).unwrap();
        let path = ledger.path(&root.sha256, "json");
        fs::write(
            &path,
            serde_json::to_vec(
                &serde_json::json!({"schema":1,"sha256":root.sha256,"persistent":true}),
            )
            .unwrap(),
        )
        .unwrap();
        let (_, migrated) = ledger.prepare(true).unwrap();
        assert_eq!(migrated.sha256, root.sha256);
        assert_eq!(migrated.schema, 2);
        assert!(migrated.lifecycle.created_at.is_some());
        fs::write(
            &path,
            serde_json::to_vec(
                &serde_json::json!({"schema":99,"sha256":root.sha256,"persistent":true}),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(ledger.records().is_err());
    }

    #[test]
    fn redaction_choice_persists_across_runs_and_can_be_reversed() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        assert!(!ledger.redaction(None).unwrap());
        assert!(ledger.redaction(Some(true)).unwrap());
        drop(ledger);
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        assert!(ledger.redaction(None).unwrap());
        assert!(!ledger.redaction(Some(false)).unwrap());
        assert!(!ledger.redaction(None).unwrap());
    }
    #[test]
    fn request_retention_limit_persists_and_preserves_redaction() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        assert_eq!(
            ledger.preferences(None, None).unwrap(),
            (false, Some(25_000_000))
        );
        assert_eq!(
            ledger.preferences(Some(true), Some(None)).unwrap(),
            (true, None)
        );
        drop(ledger);
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        assert_eq!(ledger.preferences(None, None).unwrap(), (true, None));
        assert_eq!(
            ledger.preferences(None, Some(Some(25_000_000))).unwrap(),
            (true, Some(25_000_000))
        );
        assert!(ledger.preferences(None, Some(Some(0))).is_err());
    }
    #[test]
    fn corrupt_newest_privacy_falls_back_and_next_save_uses_a_fresh_generation() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        ledger.redaction(Some(true)).unwrap();
        ledger.redaction(Some(false)).unwrap();
        fs::write(
            directory.path().join("privacy-00000000000000000002.json"),
            b"broken",
        )
        .unwrap();
        assert!(ledger.redaction(None).unwrap());
        assert!(!ledger.redaction(Some(false)).unwrap());
        drop(ledger);
        assert!(
            !RootLedger::open(directory.path().into())
                .unwrap()
                .redaction(None)
                .unwrap()
        );
    }
    #[test]
    fn persistent_root_is_reused_and_excluded_from_automatic_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(directory.path().into()).unwrap();
        let (_, first) = ledger.prepare(true).unwrap();
        assert!(ledger.path(&first.sha256, "key").is_file());
        assert_eq!(ledger.prepare(true).unwrap().1.sha256, first.sha256);
        ledger
            .cleanup_ephemeral(&FakeTrust { fail: false })
            .unwrap();
        assert_eq!(ledger.records().unwrap().len(), 1);
        assert!(RootLedger::open(directory.path().into()).is_err());
    }
}
