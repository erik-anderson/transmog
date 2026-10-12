//! Signed artifact validation and local reminder persistence, independent of Tauri.

use std::{fs, io::Write as _, path::PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const REMINDER_DAYS_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

pub(crate) fn is_owned_update_file(name: &str) -> bool {
    let generation = name
        .strip_prefix("update-preferences.")
        .and_then(|name| name.strip_suffix(".json"))
        .or_else(|| {
            name.strip_prefix(".transmog-update-")
                .and_then(|name| name.strip_suffix(".tmp"))
        });
    generation
        .is_some_and(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Verify the final bytes and the signed version, not merely the unsigned manifest.
pub(crate) fn verify_artifact(
    bytes: &[u8],
    signature: &str,
    public_key: &str,
    version: &str,
) -> Result<(), String> {
    use base64::Engine as _;
    let decode = |value: &str| {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .map_err(|_| "Invalid updater signing data.")?;
        String::from_utf8(bytes).map_err(|_| "Invalid updater signing data.")
    };
    let key = minisign_verify::PublicKey::decode(&decode(public_key)?)
        .map_err(|_| "Invalid updater public key.")?;
    let signature = minisign_verify::Signature::decode(&decode(signature)?)
        .map_err(|_| "Invalid updater signature.")?;
    key.verify(bytes, &signature, false).map_err(
        |_| "The updater signature does not match the installer or embedded public key.",
    )?;
    let signed = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"));
    if semver::Version::parse(version).is_err()
        || signed.and_then(|value| semver::Version::parse(value).ok())
            != semver::Version::parse(version).ok()
    {
        return Err("The signed installer version differs from the release manifest.".to_owned());
    }
    Ok(())
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Reminder {
    pub(crate) remind_after_unix_ms: u64,
    #[serde(default = "enabled_by_default")]
    pub(crate) automatic_checks_enabled: bool,
}
const fn enabled_by_default() -> bool {
    true
}
impl Default for Reminder {
    fn default() -> Self {
        Self {
            remind_after_unix_ms: 0,
            automatic_checks_enabled: true,
        }
    }
}

pub(crate) fn automatic_check_permitted(preferences: Reminder, debug: bool, now: u64) -> bool {
    preferences.automatic_checks_enabled && !debug && preferences.remind_after_unix_ms <= now
}

/// Installer-safe preference commands run before GUI or singleton initialization.
pub(crate) fn preference_maintenance_exit_code() -> Option<i32> {
    let arguments = std::env::args_os()
        .skip(1)
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let argument = arguments.iter().find(|value| {
        value.as_str() == "--get-automatic-update-checks"
            || value.starts_with("--set-automatic-update-checks=")
    })?;
    if arguments.len() != 1 {
        return Some(2);
    }
    let Some(root) = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    else {
        return Some(2);
    };
    let mut store = ReminderStore::load(root.join("Transmog"));
    if argument == "--get-automatic-update-checks" {
        return Some(if store.reminder.automatic_checks_enabled {
            0
        } else {
            3
        });
    }
    let enabled = match argument.strip_prefix("--set-automatic-update-checks=") {
        Some("0") => false,
        Some("1") => true,
        _ => return Some(2),
    };
    Some(if store.set_automatic_checks(enabled).is_ok() {
        0
    } else {
        2
    })
}

/// Create-new generations avoid Windows' non-replacing rename and stale Settings writes.
pub(crate) struct ReminderStore {
    root: PathBuf,
    next_generation: u64,
    pub(crate) reminder: Reminder,
}

impl ReminderStore {
    pub(crate) fn load(root: PathBuf) -> Self {
        let listing = fs::read_dir(&root);
        let unavailable = listing
            .as_ref()
            .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound);
        let mut files = listing
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let generation = name
                    .to_str()?
                    .strip_prefix("update-preferences.")?
                    .strip_suffix(".json")?
                    .parse::<u64>()
                    .ok()?;
                Some((generation, entry.path()))
            })
            .collect::<Vec<_>>();
        files.sort_unstable_by_key(|file| std::cmp::Reverse(file.0));
        let next_generation = files.first().map_or(1, |file| file.0.saturating_add(1));
        let mut invalid_newest = unavailable;
        let reminder = files
            .iter()
            .find_map(|(_, path)| {
                let result = fs::metadata(path)
                    .ok()
                    .filter(|metadata| metadata.len() <= 4_096)
                    .and_then(|_| fs::read(path).ok())
                    .and_then(|bytes| serde_json::from_slice::<Reminder>(&bytes).ok());
                if result.is_none() {
                    invalid_newest = true;
                }
                result.map(|mut preferences| {
                    if invalid_newest {
                        preferences.automatic_checks_enabled = false;
                    }
                    preferences
                })
            })
            .unwrap_or_else(|| Reminder {
                automatic_checks_enabled: files.is_empty() && !unavailable,
                ..Reminder::default()
            });
        Self {
            root,
            next_generation,
            reminder,
        }
    }

    pub(crate) fn snooze(&mut self, now: u64) -> Result<u64, String> {
        let deadline = now.saturating_add(REMINDER_DAYS_MS);
        self.persist(|reminder| Reminder {
            remind_after_unix_ms: deadline,
            ..reminder
        })?;
        Ok(deadline)
    }
    pub(crate) fn set_automatic_checks(&mut self, enabled: bool) -> Result<(), String> {
        self.persist(|reminder| Reminder {
            automatic_checks_enabled: enabled,
            ..reminder
        })
    }
    fn persist(&mut self, change: impl Fn(Reminder) -> Reminder) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|_| "Update preferences could not be saved.")?;
        for _ in 0..8 {
            // The installer and an open desktop can write independently. Merge the
            // unchanged field from the latest published generation before saving.
            let latest = Self::load(self.root.clone());
            self.next_generation = self.next_generation.max(latest.next_generation);
            let reminder = change(latest.reminder);
            let encoded = serde_json::to_vec(&reminder)
                .map_err(|_| "Update preferences could not be encoded.")?;
            let generation = self.next_generation;
            self.next_generation = generation
                .checked_add(1)
                .ok_or("Update preference generations are exhausted.")?;
            let temporary = self.root.join(format!(".transmog-update-{generation}.tmp"));
            let destination = self
                .root
                .join(format!("update-preferences.{generation}.json"));
            let mut file = match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    return Err("Update preferences could not be saved. Try again.".to_owned());
                }
            };
            let result = file
                .write_all(&encoded)
                .and_then(|()| file.sync_all())
                .and_then(|()| {
                    drop(file);
                    fs::rename(&temporary, &destination)
                });
            if result.is_err() {
                let _ = fs::remove_file(temporary);
                return Err("Update preferences could not be saved. Try again.".to_owned());
            }
            self.reminder = reminder;
            let mut published = fs::read_dir(&self.root)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let generation = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_prefix("update-preferences."))
                        .and_then(|name| name.strip_suffix(".json"))
                        .and_then(|value| value.parse::<u64>().ok())?;
                    Some((generation, entry.path()))
                })
                .collect::<Vec<_>>();
            published.sort_unstable_by_key(|file| std::cmp::Reverse(file.0));
            for (_, path) in published.into_iter().skip(3) {
                let _ = fs::remove_file(path);
            }
            return Ok(());
        }
        Err("Update preferences are busy. Try again.".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_choice_migrates_and_survives_snooze_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ReminderStore::load(root.path().to_owned());
        assert!(automatic_check_permitted(store.reminder, false, 0));
        assert!(!automatic_check_permitted(store.reminder, true, 0));
        store.set_automatic_checks(false).unwrap();
        let deadline = store.snooze(10).unwrap();
        let mut reloaded = ReminderStore::load(root.path().to_owned());
        assert!(!reloaded.reminder.automatic_checks_enabled);
        assert_eq!(reloaded.reminder.remind_after_unix_ms, deadline);
        assert!(!automatic_check_permitted(
            reloaded.reminder,
            false,
            deadline + 1
        ));
        reloaded.set_automatic_checks(true).unwrap();
        assert!(!automatic_check_permitted(
            reloaded.reminder,
            false,
            deadline - 1
        ));
        assert!(automatic_check_permitted(
            reloaded.reminder,
            false,
            deadline
        ));
        let legacy: Reminder = serde_json::from_str(r#"{"remindAfterUnixMs":42}"#).unwrap();
        assert!(legacy.automatic_checks_enabled);
        assert_eq!(legacy.remind_after_unix_ms, 42);
    }

    #[test]
    fn failed_choice_save_and_stale_temporary_file_do_not_lose_preferences() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ReminderStore::load(root.path().to_owned());
        fs::write(
            root.path().join(".transmog-update-1.tmp"),
            "owned interrupted save",
        )
        .unwrap();
        store.set_automatic_checks(false).unwrap();
        assert!(
            !ReminderStore::load(root.path().to_owned())
                .reminder
                .automatic_checks_enabled
        );
        let invalid = tempfile::NamedTempFile::new().unwrap();
        let mut blocked = ReminderStore::load(invalid.path().to_owned());
        assert!(blocked.set_automatic_checks(false).is_err());
        assert!(!blocked.reminder.automatic_checks_enabled);
    }

    #[test]
    fn independent_writers_merge_choices_and_keep_three_recovery_generations() {
        let root = tempfile::tempdir().unwrap();
        let mut desktop = ReminderStore::load(root.path().to_owned());
        let mut installer = ReminderStore::load(root.path().to_owned());
        let deadline = desktop.snooze(100).unwrap();
        installer.set_automatic_checks(false).unwrap();
        assert_eq!(installer.reminder.remind_after_unix_ms, deadline);
        desktop.snooze(200).unwrap();
        assert!(!desktop.reminder.automatic_checks_enabled);
        for generation in 4..7 {
            fs::write(
                root.path()
                    .join(format!(".transmog-update-{generation}.tmp")),
                "interrupted",
            )
            .unwrap();
        }
        installer.set_automatic_checks(true).unwrap();
        let files = fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .count();
        assert_eq!(files, 3);
        assert!(
            ReminderStore::load(root.path().to_owned())
                .reminder
                .automatic_checks_enabled
        );
    }

    #[test]
    fn signatures_bind_the_bytes_key_and_semantic_version() {
        let bytes = include_bytes!("../tests/fixtures/updater-artifact.txt");
        let signature = include_str!("../tests/fixtures/updater-artifact.txt.sig");
        let key = include_str!("../tests/fixtures/updater-public.txt");
        assert!(verify_artifact(bytes, signature, key, "0.2.0").is_ok());
        assert!(verify_artifact(b"tampered", signature, key, "0.2.0").is_err());
        assert!(verify_artifact(bytes, signature, key, "0.2.1").is_err());
        assert!(verify_artifact(bytes, signature, key, "0.2.0.1").is_err());
        assert!(verify_artifact(bytes, "invalid", key, "0.2.0").is_err());
    }

    #[test]
    fn reminders_survive_restart_and_recover_from_invalid_newest_generation() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ReminderStore::load(root.path().to_owned());
        let deadline = store.snooze(10).unwrap();
        assert_eq!(deadline, 10 + REMINDER_DAYS_MS);
        fs::write(root.path().join("update-preferences.2.json"), "broken").unwrap();
        let mut recovered = ReminderStore::load(root.path().to_owned());
        assert_eq!(recovered.reminder.remind_after_unix_ms, deadline);
        assert!(!recovered.reminder.automatic_checks_enabled);
        recovered.snooze(20).unwrap();
        assert_eq!(
            ReminderStore::load(root.path().to_owned())
                .reminder
                .remind_after_unix_ms,
            20 + REMINDER_DAYS_MS
        );
    }

    #[test]
    fn failed_saves_do_not_claim_a_reminder_was_saved() {
        let root = tempfile::NamedTempFile::new().unwrap();
        let mut store = ReminderStore::load(root.path().to_owned());
        assert!(store.snooze(10).is_err());
        assert_eq!(store.reminder.remind_after_unix_ms, 0);
    }

    #[test]
    fn cleanup_only_recognizes_owned_update_state_names() {
        assert!(is_owned_update_file("update-preferences.1.json"));
        assert!(is_owned_update_file(".transmog-update-2.tmp"));
        for name in [
            "update-preferences.notes.json",
            "update-preferences..json",
            "update-preferences.1.json.backup",
            ".transmog-update-notes.tmp",
        ] {
            assert!(!is_owned_update_file(name));
        }
    }
}
