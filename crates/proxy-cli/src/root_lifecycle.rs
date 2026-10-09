//! Public recovery facts only. No PEM key bytes ever enter an ownership record.
use serde::{Deserialize, Serialize};
use std::{
    io,
    time::{SystemTime, UNIX_EPOCH},
};
use transmog_tls::ProxyCa;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RootLifecycle {
    #[default]
    Prepared,
    InstallationRequested,
    Installed,
    Manual,
    PersistentIdle,
    CaptureFinished,
    CleanupPending,
    Retired,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum KeyStorage {
    #[default]
    MemoryOnly,
    ProtectedFile,
    Removed,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Lifecycle {
    pub(crate) state: RootLifecycle,
    pub(crate) key_storage: KeyStorage,
    pub(crate) trust_store: String,
    pub(crate) created_at: Option<u64>,
    pub(crate) expires_at: Option<u64>,
    pub(crate) installed_at: Option<u64>,
    pub(crate) last_used_at: Option<u64>,
    pub(crate) run_id: Option<String>,
    pub(crate) finished_at: Option<u64>,
    pub(crate) outcome: Option<String>,
    pub(crate) retired: bool,
    pub(crate) cleanup_attempts: u32,
    pub(crate) last_cleanup_at: Option<u64>,
    pub(crate) last_cleanup_error: Option<String>,
}
impl Lifecycle {
    pub(crate) fn begin(&mut self, ca: &ProxyCa, persistent: bool) -> io::Result<()> {
        let now = millis();
        self.state = RootLifecycle::Prepared;
        self.finished_at = None;
        self.outcome = None;
        self.key_storage = if persistent {
            KeyStorage::ProtectedFile
        } else {
            KeyStorage::MemoryOnly
        };
        self.trust_store = store().into();

        self.last_used_at = Some(now);
        self.run_id = Some(format!(
            "{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id()
        ));
        let epoch = boring::asn1::Asn1Time::from_unix(0).map_err(io::Error::other)?;
        let issued = epoch
            .diff(ca.certificate().not_before())
            .map_err(io::Error::other)?;
        let issued_seconds = i64::from(issued.days) * 86400 + i64::from(issued.secs);
        self.created_at.get_or_insert(
            u64::try_from(issued_seconds)
                .map_err(io::Error::other)?
                .saturating_mul(1000),
        );
        let expires = epoch
            .diff(ca.certificate().not_after())
            .map_err(io::Error::other)?;
        let seconds = i64::from(expires.days)
            .checked_mul(86400)
            .and_then(|days| days.checked_add(i64::from(expires.secs)))
            .ok_or_else(|| io::Error::other("Root expiry is out of range"))?;
        self.expires_at = Some(
            u64::try_from(seconds)
                .map_err(io::Error::other)?
                .saturating_mul(1000),
        );
        Ok(())
    }
    pub(crate) fn valid(&self) -> bool {
        self.outcome
            .as_ref()
            .is_none_or(|outcome| outcome.len() <= 64)
            && self.trust_store.len() <= 128
            && self.run_id.as_ref().is_none_or(|id| id.len() <= 128)
            && self
                .last_cleanup_error
                .as_ref()
                .is_none_or(|error| error.len() <= 2048)
            && [
                self.created_at,
                self.expires_at,
                self.installed_at,
                self.last_used_at,
                self.last_cleanup_at,
                self.finished_at,
            ]
            .iter()
            .all(|time| time.is_none_or(|time| time <= 253_402_300_799_999))
    }
}
pub(crate) fn reusable(ca: &ProxyCa) -> io::Result<bool> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    let now = boring::asn1::Asn1Time::from_unix(i64::try_from(seconds).map_err(io::Error::other)?)
        .map_err(io::Error::other)?;
    let horizon = boring::asn1::Asn1Time::from_unix(
        i64::try_from(seconds.saturating_add(7 * 86400)).map_err(io::Error::other)?,
    )
    .map_err(io::Error::other)?;
    Ok(ca
        .certificate()
        .not_before()
        .compare(&now)
        .map_err(io::Error::other)?
        != std::cmp::Ordering::Greater
        && ca
            .certificate()
            .not_after()
            .compare(&horizon)
            .map_err(io::Error::other)?
            == std::cmp::Ordering::Greater)
}
pub(crate) fn millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
fn store() -> &'static str {
    #[cfg(windows)]
    {
        "Windows current-user Root store"
    }
    #[cfg(target_os = "macos")]
    {
        "macOS current-user login keychain"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "Linux system trust store (explicit sudo consent)"
    }
}
