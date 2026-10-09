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

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Reminder {
    pub(crate) remind_after_unix_ms: u64,
}

/// Create-new generations avoid Windows' non-replacing rename and stale Settings writes.
pub(crate) struct ReminderStore {
    root: PathBuf,
    next_generation: u64,
    pub(crate) reminder: Reminder,
}

impl ReminderStore {
    pub(crate) fn load(root: PathBuf) -> Self {
        let mut files = fs::read_dir(&root)
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
        let reminder = files
            .iter()
            .find_map(|(_, path)| {
                if fs::metadata(path).ok()?.len() > 4_096 {
                    return None;
                }
                serde_json::from_slice(&fs::read(path).ok()?).ok()
            })
            .unwrap_or_default();
        Self {
            root,
            next_generation,
            reminder,
        }
    }

    pub(crate) fn snooze(&mut self, now: u64) -> Result<u64, String> {
        let reminder = Reminder {
            remind_after_unix_ms: now.saturating_add(REMINDER_DAYS_MS),
        };
        fs::create_dir_all(&self.root).map_err(|_| "The update reminder could not be saved.")?;
        let temporary = self
            .root
            .join(format!(".transmog-update-{}.tmp", self.next_generation));
        let destination = self
            .root
            .join(format!("update-preferences.{}.json", self.next_generation));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(&reminder)?)?;
            file.sync_all()?;
            fs::rename(&temporary, destination)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
            return Err("The update reminder could not be saved. Try again.".to_owned());
        }
        self.reminder = reminder;
        self.next_generation += 1;
        // Keep the newest three generations. Unknown files are never removed.
        for entry in fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let name = entry.file_name();
            if let Some(generation) = name
                .to_str()
                .and_then(|name| name.strip_prefix("update-preferences."))
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|name| name.parse::<u64>().ok())
                && generation.saturating_add(3) < self.next_generation
            {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(reminder.remind_after_unix_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
