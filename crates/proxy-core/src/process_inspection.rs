//! Exact executable-name selection for newly admitted downstream connections.
//!
//! Process attribution is diagnostic, not an authorization boundary. Unresolved
//! and remote callers bypass message inspection whenever a name filter is active.
//! CONNECT tunnels still retain connection summaries without headers or contents.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ClientIdentity;

/// Maximum names retained in either process list.
pub const MAX_PROCESS_NAMES: usize = 256;
/// Maximum UTF-8 bytes in one configured executable name.
pub const MAX_PROCESS_NAME_BYTES: usize = 260;

/// Which caller processes may enter inspection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessInspectionMode {
    /// Inspect every caller, including unresolved and remote callers.
    #[default]
    All,
    /// Inspect only resolved callers named in the include list.
    Only,
    /// Inspect resolved callers other than those in the exclude list.
    Except,
}

/// Saved process-selection choices; inactive lists remain available for editing.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ProcessInspectionPolicy {
    /// Active selection mode.
    pub mode: ProcessInspectionMode,
    /// Exact executable names selected by [`ProcessInspectionMode::Only`].
    pub include: Vec<String>,
    /// Exact executable names selected by [`ProcessInspectionMode::Except`].
    pub exclude: Vec<String>,
}

/// An invalid process-selection choice, without echoing authored names.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ProcessInspectionPolicyError {
    /// An active include or exclude mode has no names.
    #[error("Choose at least one executable name for the selected process mode.")]
    EmptyActiveList,
    /// A configured list exceeds its bounded entry count.
    #[error("Each process list can contain at most {MAX_PROCESS_NAMES} executable names.")]
    TooManyNames,
    /// A name is empty, too long, contains a path, wildcard, or control character.
    #[error(
        "Use executable file names without paths, wildcards, or control characters (at most {MAX_PROCESS_NAME_BYTES} bytes each)."
    )]
    InvalidName,
}

impl ProcessInspectionPolicy {
    /// Trims, normalizes and deduplicates both lists, retaining their order.
    ///
    /// Windows matching ignores case and appends `.exe` to extensionless names.
    /// Names with extensions retain their exact basename. Other platforms match
    /// executable file names exactly, including their case.
    ///
    /// # Errors
    /// Returns a bounded-list/name error or an empty active-list error.
    pub fn validated(mut self) -> Result<Self, ProcessInspectionPolicyError> {
        self.include = validate_names(self.include)?;
        self.exclude = validate_names(self.exclude)?;
        if matches!(self.mode, ProcessInspectionMode::Only) && self.include.is_empty()
            || matches!(self.mode, ProcessInspectionMode::Except) && self.exclude.is_empty()
        {
            return Err(ProcessInspectionPolicyError::EmptyActiveList);
        }
        Ok(self)
    }

    /// Decides inspection once for a downstream caller identity snapshot.
    #[must_use]
    pub fn should_inspect(&self, identity: &ClientIdentity) -> bool {
        if self.mode == ProcessInspectionMode::All {
            return true;
        }
        let ClientIdentity::LocalProcess {
            name: Some(name), ..
        } = identity
        else {
            return false;
        };
        // Authored names are trimmed for convenience. An observed basename
        // containing surrounding whitespace is a different executable, not an
        // alias for the trimmed input, and cannot be matched by these rules.
        if name != name.trim() {
            return false;
        }
        let Ok(name) = normalize_name(name) else {
            return false;
        };
        match self.mode {
            ProcessInspectionMode::All => true,
            ProcessInspectionMode::Only => self.include.contains(&name),
            ProcessInspectionMode::Except => !self.exclude.contains(&name),
        }
    }
}

fn validate_names(names: Vec<String>) -> Result<Vec<String>, ProcessInspectionPolicyError> {
    if names.len() > MAX_PROCESS_NAMES {
        return Err(ProcessInspectionPolicyError::TooManyNames);
    }
    let mut normalized: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let name = normalize_name(&name)?;
        if !normalized.contains(&name) {
            normalized.push(name);
        }
    }
    Ok(normalized)
}

fn normalize_name(name: &str) -> Result<String, ProcessInspectionPolicyError> {
    if name.len() > MAX_PROCESS_NAME_BYTES
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(ProcessInspectionPolicyError::InvalidName);
    }
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.ends_with('.') {
        return Err(ProcessInspectionPolicyError::InvalidName);
    }
    #[cfg(windows)]
    let name = {
        let mut lowered = name.to_lowercase();
        if !lowered.contains('.') {
            lowered.push_str(".exe");
        }
        lowered
    };
    #[cfg(not(windows))]
    let name = name.to_owned();
    if name.is_empty() || cfg!(windows) && name == ".exe" || name.len() > MAX_PROCESS_NAME_BYTES {
        return Err(ProcessInspectionPolicyError::InvalidName);
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> ClientIdentity {
        ClientIdentity::LocalProcess {
            pid: 42,
            name: Some(name.into()),
        }
    }

    #[test]
    fn defaults_inspect_all_identities() {
        let policy = ProcessInspectionPolicy::default();
        for identity in [
            named("browser.exe"),
            ClientIdentity::LocalUnknown,
            ClientIdentity::Remote,
            ClientIdentity::LocalProcess { pid: 1, name: None },
        ] {
            assert!(policy.should_inspect(&identity));
        }
        assert_eq!(
            serde_json::from_str::<ProcessInspectionPolicy>("{}").unwrap(),
            policy
        );
    }

    #[test]
    fn both_filters_skip_unresolved_and_remote_callers_and_preserve_inactive_lists() {
        for mode in [ProcessInspectionMode::Only, ProcessInspectionMode::Except] {
            let policy = ProcessInspectionPolicy {
                mode,
                include: vec![" browser.exe ".into(), "browser.exe".into()],
                exclude: vec!["other.exe".into()],
            }
            .validated()
            .unwrap();
            assert_eq!(policy.include.len(), 1);
            assert_eq!(policy.exclude.len(), 1);
            assert!(policy.should_inspect(&named("browser.exe")));
            assert!(!policy.should_inspect(&named("other.exe")));
            for unknown in [
                ClientIdentity::LocalUnknown,
                ClientIdentity::Remote,
                ClientIdentity::LocalProcess {
                    pid: 42,
                    name: None,
                },
                named(""),
            ] {
                assert!(!policy.should_inspect(&unknown));
            }
        }
    }

    #[test]
    fn invalid_names_and_unbounded_lists_are_rejected_even_when_inactive() {
        for name in [
            "",
            " ",
            "../browser",
            "C:\\browser.exe",
            "browser*",
            "bro?ser",
            "bro\nser",
            ".",
            "..",
            ".exe",
        ] {
            let result = ProcessInspectionPolicy {
                include: vec![name.into()],
                ..Default::default()
            }
            .validated();
            if name == ".exe" && !cfg!(windows) {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err(), "{name:?}");
            }
        }
        assert!(
            ProcessInspectionPolicy {
                include: vec!["a".repeat(MAX_PROCESS_NAME_BYTES + 1)],
                ..Default::default()
            }
            .validated()
            .is_err()
        );
        assert!(
            ProcessInspectionPolicy {
                include: vec!["a".into(); MAX_PROCESS_NAMES + 1],
                ..Default::default()
            }
            .validated()
            .is_err()
        );
        for mode in [ProcessInspectionMode::Only, ProcessInspectionMode::Except] {
            assert_eq!(
                ProcessInspectionPolicy {
                    mode,
                    ..Default::default()
                }
                .validated(),
                Err(ProcessInspectionPolicyError::EmptyActiveList)
            );
        }
    }

    #[test]
    fn platform_filename_matching_is_exact() {
        let policy = ProcessInspectionPolicy {
            mode: ProcessInspectionMode::Only,
            include: vec!["Browser.exe".into()],
            exclude: vec![],
        }
        .validated()
        .unwrap();
        assert!(policy.should_inspect(&named("Browser.exe")));
        assert_eq!(policy.should_inspect(&named("browser")), cfg!(windows));
        assert_eq!(policy.should_inspect(&named("BROWSER.EXE")), cfg!(windows));
        assert!(!policy.should_inspect(&named("Browser.exe.extra")));
        assert!(!policy.should_inspect(&named("/path/Browser.exe")));
        let saved = serde_json::to_value(&policy).unwrap();
        assert_eq!(saved["mode"], "only");
        assert_eq!(
            serde_json::from_value::<ProcessInspectionPolicy>(saved).unwrap(),
            policy
        );
    }

    #[test]
    fn observed_whitespace_is_not_an_executable_alias() {
        for mode in [ProcessInspectionMode::Only, ProcessInspectionMode::Except] {
            let policy = ProcessInspectionPolicy {
                mode,
                include: vec![" browser.exe ".into()],
                exclude: vec!["other.exe".into()],
            }
            .validated()
            .unwrap();
            assert!(policy.should_inspect(&named("browser.exe")));
            for basename in [" browser.exe", "browser.exe ", " browser.exe "] {
                assert!(!policy.should_inspect(&named(basename)));
                assert!(ProcessInspectionPolicy::default().should_inspect(&named(basename)));
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_preserves_canonical_filenames_and_bounds_normalized_names() {
        let policy = ProcessInspectionPolicy {
            include: vec![
                " Browser ".into(),
                "BROWSER.EXE".into(),
                "worker.COM".into(),
            ],
            ..Default::default()
        }
        .validated()
        .unwrap();
        assert_eq!(policy.include, ["browser.exe", "worker.com"]);
        assert_eq!(policy.clone().validated().unwrap(), policy);
        let dotted = ProcessInspectionPolicy {
            mode: ProcessInspectionMode::Only,
            include: vec!["service.host".into()],
            ..Default::default()
        }
        .validated()
        .unwrap();
        assert!(dotted.should_inspect(&named("Service.Host")));
        assert!(!dotted.should_inspect(&named("Service.Host.exe")));
        let com = ProcessInspectionPolicy {
            mode: ProcessInspectionMode::Only,
            include: vec!["worker.COM".into()],
            ..Default::default()
        }
        .validated()
        .unwrap();
        assert!(com.should_inspect(&named("Worker.com")));
        assert!(!com.should_inspect(&named("Worker.com.exe")));
        assert!(
            ProcessInspectionPolicy {
                include: vec!["x".repeat(MAX_PROCESS_NAME_BYTES - 3)],
                ..Default::default()
            }
            .validated()
            .is_err()
        );
        assert!(
            ProcessInspectionPolicy {
                include: vec!["x".repeat(MAX_PROCESS_NAME_BYTES - 4)],
                ..Default::default()
            }
            .validated()
            .is_ok()
        );
    }
}
