//! Native update checks, verified staging, and session-scoped installation consent.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{AppHandle, ipc::Channel};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::update_policy::ReminderStore;

const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatus {
    pub(crate) phase: &'static str,
    pub(crate) current_version: String,
    pub(crate) version: Option<String>,
    pub(crate) notes: String,
    pub(crate) message: String,
    pub(crate) downloaded_bytes: u64,
    pub(crate) total_bytes: Option<u64>,
    pub(crate) scheduled: bool,
    pub(crate) suppressed: bool,
    pub(crate) remind_after_unix_ms: u64,
}

struct Inner {
    status: UpdateStatus,
    candidate: Option<Update>,
    staged: Option<Vec<u8>>,
    reminder: ReminderStore,
    close_channel: Option<Channel<()>>,
}

pub(crate) struct UpdateCoordinator {
    inner: Mutex<Inner>,
    operation: tokio::sync::Mutex<()>,
    cancelled: AtomicBool,
    cancel: tokio::sync::watch::Sender<u64>,
}

impl UpdateCoordinator {
    pub(crate) fn new(root: PathBuf) -> Self {
        let reminder = ReminderStore::load(root);
        let release: serde_json::Value =
            serde_json::from_str(include_str!("../../../release-version.json"))
                .expect("release metadata");
        Self {
            inner: Mutex::new(Inner {
                status: UpdateStatus {
                    phase: "idle",
                    current_version: release["version"]
                        .as_str()
                        .expect("product version")
                        .to_owned(),
                    remind_after_unix_ms: reminder.reminder.remind_after_unix_ms,
                    ..UpdateStatus::default()
                },
                candidate: None,
                staged: None,
                reminder,
                close_channel: None,
            }),
            operation: tokio::sync::Mutex::new(()),
            cancelled: AtomicBool::new(false),
            cancel: tokio::sync::watch::channel(0).0,
        }
    }

    pub(crate) fn status(&self) -> UpdateStatus {
        self.inner.lock().unwrap().status.clone()
    }

    pub(crate) fn watch_close(&self, channel: Channel<()>) {
        self.inner.lock().unwrap().close_channel = Some(channel);
    }

    /// The frontend resolves drafts before acknowledging a normal close with a staged update.
    pub(crate) fn request_scheduled_close(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.status.scheduled
            && inner.staged.is_some()
            && inner
                .close_channel
                .as_ref()
                .is_some_and(|channel| channel.send(()).is_ok())
    }

    pub(crate) async fn check(
        &self,
        app: &AppHandle,
        manual: bool,
    ) -> Result<UpdateStatus, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "An update operation is already in progress.")?;
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.staged.is_some() {
                return Ok(inner.status.clone());
            }
            if !manual && (cfg!(debug_assertions) || inner.status.remind_after_unix_ms > now_ms()) {
                inner.status.suppressed = true;
                return Ok(inner.status.clone());
            }
            inner.status.phase = "checking";
            inner.status.suppressed = false;
            "Checking for a stable release…".clone_into(&mut inner.status.message);
        }
        let result = async {
            let updater = app.updater_builder()
                .target("windows-x86_64-nsis")
                .timeout(Duration::from_secs(20))
                .build().map_err(|_| "The update checker could not initialize.".to_owned())?;
            updater.check().await.map_err(|_| "The update check could not reach GitHub or read the release manifest. Try again later.".to_owned())
        }.await;
        match result {
            Ok(Some(mut update)) => {
                if !valid_download(&update.version, update.download_url.as_str()) {
                    return Err(
                        self.fail("The release contains an unexpected Windows installer URL.")
                    );
                }
                // Checking is bounded separately from an explicitly requested installer download.
                update.timeout = Some(Duration::from_secs(15 * 60));
                let mut inner = self.inner.lock().unwrap();
                inner.status.phase = "available";
                inner.status.version = Some(update.version.clone());
                inner.status.notes = update
                    .body
                    .as_deref()
                    .unwrap_or_default()
                    .chars()
                    .take(8_192)
                    .collect();
                "A new stable release is available.".clone_into(&mut inner.status.message);
                inner.candidate = Some(update);
                Ok(inner.status.clone())
            }
            Ok(None) => {
                let mut inner = self.inner.lock().unwrap();
                inner.status.phase = "idle";
                inner.status.version = None;
                inner.status.notes.clear();
                "You’re up to date with the stable release.".clone_into(&mut inner.status.message);
                inner.candidate = None;
                Ok(inner.status.clone())
            }
            Err(message) => Err(self.fail(&message)),
        }
    }

    pub(crate) async fn stage(
        self: &Arc<Self>,
        after_session: bool,
        progress: Channel<UpdateStatus>,
    ) -> Result<UpdateStatus, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "An update operation is already in progress.")?;
        let (update, mut cancellation) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.staged.is_some() {
                inner.status.scheduled = after_session;
                inner.status.phase = "ready";
                (if after_session { "The verified update will install when you quit Transmog. It will not reopen the app." } else { "The verified update is ready to install." }).clone_into(&mut inner.status.message);
                return Ok(inner.status.clone());
            }
            let update = inner
                .candidate
                .clone()
                .ok_or("Check for an available update first.")?;
            self.cancelled.store(false, Ordering::Release);
            inner.status.phase = "downloading";
            "Downloading and verifying the installer…".clone_into(&mut inner.status.message);
            inner.status.downloaded_bytes = 0;
            inner.status.total_bytes = None;
            (update, self.cancel.subscribe())
        };
        let too_large = AtomicBool::new(false);
        let mut last_progress = Instant::now();
        let download = update.download(
            |chunk, total| {
                let mut inner = self.inner.lock().unwrap();
                inner.status.downloaded_bytes =
                    inner.status.downloaded_bytes.saturating_add(chunk as u64);
                inner.status.total_bytes = total;
                if inner.status.downloaded_bytes > MAX_INSTALLER_BYTES
                    || total.is_some_and(|size| size > MAX_INSTALLER_BYTES)
                {
                    too_large.store(true, Ordering::Release);
                    self.cancel
                        .send_modify(|generation| *generation = generation.wrapping_add(1));
                }
                if last_progress.elapsed() >= Duration::from_millis(150) {
                    let _ = progress.send(inner.status.clone());
                    last_progress = Instant::now();
                }
            },
            || {},
        );
        let result = tokio::select! {
            result = download => Some(result),
            _ = cancellation.changed() => None,
        };
        if too_large.load(Ordering::Acquire) {
            return Err(self.fail("The installer exceeds the permitted download size."));
        }
        let mut inner = self.inner.lock().unwrap();
        if self.cancelled.load(Ordering::Acquire) || result.is_none() {
            inner.status.phase = "available";
            inner.status.scheduled = false;
            "Update cancelled. You can update later.".clone_into(&mut inner.status.message);
            return Ok(inner.status.clone());
        }
        if let Ok(bytes) = result.expect("checked download result") {
            inner.staged = Some(bytes);
            inner.status.phase = "ready";
            inner.status.scheduled = after_session;
            (if after_session { "The verified update will install when you quit Transmog. It will not reopen the app." } else { "The verified update is ready to install." }).clone_into(&mut inner.status.message);
            Ok(inner.status.clone())
        } else {
            inner.status.phase = "error";
            "The installer could not be downloaded or its signature/version could not be verified. Try again.".clone_into(&mut inner.status.message);
            Err(inner.status.message.clone())
        }
    }

    pub(crate) fn cancel(&self) -> UpdateStatus {
        let mut inner = self.inner.lock().unwrap();
        if inner.status.phase == "installing" {
            return inner.status.clone();
        }
        self.cancelled.store(true, Ordering::Release);
        if inner.status.phase == "downloading" {
            self.cancel
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        }
        inner.staged = None;
        inner.status.scheduled = false;
        inner.status.phase = if inner.candidate.is_some() {
            "available"
        } else {
            "idle"
        };
        "Update cancelled. You can update later.".clone_into(&mut inner.status.message);
        inner.status.clone()
    }

    pub(crate) fn remind(&self) -> Result<UpdateStatus, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "Cancel the pending update first.")?;
        let mut inner = self.inner.lock().unwrap();
        if inner.staged.is_some() {
            return Err("Cancel the scheduled update before setting a reminder.".to_owned());
        }
        inner.status.remind_after_unix_ms = inner.reminder.snooze(now_ms())?;
        inner.status.suppressed = true;
        "Automatic update prompts are paused for 30 days.".clone_into(&mut inner.status.message);
        Ok(inner.status.clone())
    }

    pub(crate) fn ensure_ready(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.staged.is_none() {
            return Err("Download and verify an update before installing it.".to_owned());
        }
        inner.status.phase = "installing";
        Ok(())
    }

    /// Called only after asynchronous capture/proxy/host cleanup has succeeded.
    pub(crate) fn install(&self, reopen: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let update = inner
            .candidate
            .as_ref()
            .ok_or("No update is available.")?
            .clone()
            .restart_after_install(reopen);
        inner.status.phase = "installing";
        let bytes = inner
            .staged
            .as_ref()
            .ok_or("No verified installer is ready.")?;
        update.install(bytes).map_err(|_| {
            "The installer could not start. Transmog remains open; try again.".to_owned()
        })
    }

    pub(crate) fn fail(&self, message: &str) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.status.phase = if inner.staged.is_some() {
            "ready"
        } else {
            "error"
        };
        message.clone_into(&mut inner.status.message);
        message.to_owned()
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn valid_download(version: &str, url: &str) -> bool {
    semver::Version::parse(version).is_ok_and(|version| {
        if !version.pre.is_empty() || !version.build.is_empty() { return false; }
        url == format!("https://github.com/erik-anderson/transmog/releases/download/v{version}/Transmog_{version}_x64-setup.exe")
    })
}

/// Read-only release qualification mode; no `WebView`, proxy, host, or installer is started.
pub(crate) fn artifact_verification_exit_code() -> Option<i32> {
    use std::io::Read as _;
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_none_or(|argument| argument != "--verify-update-artifact")
    {
        return None;
    }
    let result = (|| {
        if arguments.len() != 4 {
            return Err("Expected installer, signature, and version.".to_owned());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&arguments[1])
            .map_err(|_| "Installer unavailable.")?
            .take(MAX_INSTALLER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Installer unreadable.")?;
        if bytes.len() as u64 > MAX_INSTALLER_BYTES {
            return Err("Installer too large.".to_owned());
        }
        let mut signature = String::new();
        std::fs::File::open(&arguments[2])
            .map_err(|_| "Signature unavailable.")?
            .take(8_193)
            .read_to_string(&mut signature)
            .map_err(|_| "Signature unreadable.")?;
        if signature.len() > 8_192 {
            return Err("Signature too large.".to_owned());
        }
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .map_err(|_| "Updater configuration unavailable.")?;
        let public_key = config["plugins"]["updater"]["pubkey"]
            .as_str()
            .ok_or("Updater public key unavailable.")?;
        crate::update_policy::verify_artifact(
            &bytes,
            &signature,
            public_key,
            &arguments[3].to_string_lossy(),
        )
    })();
    if let Err(message) = &result {
        eprintln!("{message}");
    }
    Some(if result.is_ok() { 0 } else { 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installers_are_bound_to_the_repository_tag_version_and_architecture() {
        let url = "https://github.com/erik-anderson/transmog/releases/download/v1.2.3/Transmog_1.2.3_x64-setup.exe";
        assert!(valid_download("1.2.3", url));
        assert!(!valid_download("1.2.4", url));
        assert!(!valid_download(
            "1.2.3",
            &url.replace("github.com", "github.com.example.test")
        ));
        assert!(!valid_download("1.2.3", &url.replace("https:", "http:")));
        assert!(!valid_download(
            "1.2.3",
            &url.replace("transmog/releases", "other/releases")
        ));
    }

    #[test]
    fn cancellation_never_leaves_installation_consent() {
        let root = tempfile::tempdir().unwrap();
        let coordinator = UpdateCoordinator::new(root.path().to_owned());
        coordinator.inner.lock().unwrap().status.scheduled = true;
        assert!(!coordinator.cancel().scheduled);
        assert!(coordinator.ensure_ready().is_err());
    }
}
