//! Independent, bounded network profile definitions and active recipe snapshots.
use crate::{AppError, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub use transmog_core::network_profile::{
    CHROMIUM_154, CHROMIUM_REFERENCE_VERSION, NetworkProfileSettings, NetworkProfileSnapshot,
    TlsGrease, TlsVersions,
};

/// Persisted profiles; saving a definition does not implicitly activate its edits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkProfiles {
    /// Catalog revision used to reject stale writes from another window.
    pub revision: u64,
    /// Actual active recipe, retained independently of newer saved definition edits.
    pub active_profile: NetworkProfileSnapshot,
    /// User-authored copies of qualified built-in recipes.
    pub custom_profiles: Vec<NetworkProfileSnapshot>,
}

impl Default for NetworkProfiles {
    fn default() -> Self {
        Self {
            revision: 1,
            active_profile: NetworkProfileSnapshot::default(),
            custom_profiles: Vec::new(),
        }
    }
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}

impl NetworkProfiles {
    pub(crate) fn migrate(
        protocols: transmog_core::connection_policy::AllowedHttpProtocols,
        reuse: bool,
    ) -> Self {
        let mut catalog = Self::default();
        if protocols != NetworkProfileSettings::default().protocols || !reuse {
            let profile = NetworkProfileSnapshot {
                id: "custom-current-settings".into(),
                name: "Current settings".into(),
                settings: NetworkProfileSettings {
                    protocols,
                    reuse_connections: reuse,
                    ..NetworkProfileSettings::default()
                },
                ..NetworkProfileSnapshot::default()
            };
            catalog.active_profile = profile.clone();
            catalog.custom_profiles.push(profile);
        }
        catalog
    }

    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if self.revision == 0
            || self.revision > 9_007_199_254_740_990
            || self.custom_profiles.len() > 32
        {
            return Err(invalid(
                "Network profiles exceed their supported count or revision.",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for profile in &self.custom_profiles {
            validate_custom(profile)?;
            if !ids.insert(&profile.id) || !names.insert(profile.name.to_lowercase()) {
                return Err(invalid(
                    "Choose a unique name for each custom network profile.",
                ));
            }
        }
        if self.active_profile.id == CHROMIUM_154 {
            if self.active_profile != NetworkProfileSnapshot::default() {
                return Err(invalid(
                    "The Chromium 154 built-in profile cannot be edited.",
                ));
            }
        } else {
            validate_custom(&self.active_profile)?;
            if !self.custom_profiles.iter().any(|profile| {
                profile.id == self.active_profile.id
                    && profile.revision >= self.active_profile.revision
                    && (profile.revision != self.active_profile.revision
                        || profile == &self.active_profile)
            }) {
                return Err(invalid(
                    "Use another profile before deleting the active profile.",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn edited(
        &self,
        expected_revision: u64,
        mut profiles: Vec<NetworkProfileSnapshot>,
        use_profile: Option<&str>,
    ) -> Result<Self, AppError> {
        if expected_revision != self.revision {
            return Err(invalid(
                "Network profiles changed in another window. Reload profiles before saving; your draft is still available.",
            ));
        }
        for profile in &mut profiles {
            profile.name = profile.name.trim().to_owned();
            profile.revision = match self
                .custom_profiles
                .iter()
                .find(|current| current.id == profile.id)
            {
                Some(current)
                    if current.name == profile.name
                        && current.settings == profile.settings
                        && current.base_profile == profile.base_profile =>
                {
                    current.revision
                }
                Some(current) => current
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("Network profile revision is exhausted."))?,
                None => 1,
            };
        }
        let active_profile = match use_profile {
            Some(CHROMIUM_154) => NetworkProfileSnapshot::default(),
            Some(id) => profiles
                .iter()
                .find(|profile| profile.id == id)
                .cloned()
                .ok_or_else(|| invalid("Choose a saved network profile to use."))?,
            None => self.active_profile.clone(),
        };
        let catalog = Self {
            revision: self
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("Network profile catalog revision is exhausted."))?,
            active_profile,
            custom_profiles: profiles,
        };
        catalog.validate()?;
        Ok(catalog)
    }
}

fn validate_custom(profile: &NetworkProfileSnapshot) -> Result<(), AppError> {
    if !profile.id.starts_with("custom-")
        || profile.id.len() > 80
        || profile.id.len() <= 7
        || !profile
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || profile.name.is_empty()
        || profile.name.chars().count() > 80
        || profile.name.trim() != profile.name
        || profile.name.chars().any(char::is_control)
        || profile.name.eq_ignore_ascii_case("Chromium 154")
        || profile.revision == 0
        || profile.revision > 9_007_199_254_740_990
        || profile.base_profile != CHROMIUM_154
    {
        return Err(invalid(
            "Custom profiles need a unique name of up to 80 characters and a supported Chromium baseline.",
        ));
    }
    profile.settings.validate().map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> NetworkProfileSnapshot {
        NetworkProfileSnapshot {
            id: "custom-no-h3".into(),
            name: "No H3".into(),
            settings: NetworkProfileSettings {
                protocols: transmog_core::connection_policy::AllowedHttpProtocols {
                    http3: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn save_only_retains_active_revision_and_activation_is_explicit() {
        let initial = NetworkProfiles::default();
        let selected = initial
            .edited(1, vec![custom()], Some("custom-no-h3"))
            .unwrap();
        let mut changed = custom();
        changed.settings.compatibility_ciphers = false;
        let saved = selected.edited(2, vec![changed], None).unwrap();
        assert_eq!(saved.active_profile, selected.active_profile);
        assert_eq!(saved.custom_profiles[0].revision, 2);
        let applied = saved
            .edited(3, saved.custom_profiles.clone(), Some("custom-no-h3"))
            .unwrap();
        assert_eq!(applied.active_profile, saved.custom_profiles[0]);
        assert!(
            saved
                .edited(2, saved.custom_profiles.clone(), None)
                .is_err()
        );
        assert!(saved.edited(3, vec![], None).is_err());
        assert!(saved.edited(3, vec![], Some(CHROMIUM_154)).is_ok());
    }

    #[test]
    fn built_in_and_combinations_are_validated_without_banning_blocked_egress() {
        let mut invalid = custom();
        invalid.settings.protocols.http3 = true;
        invalid.settings.tls_versions = TlsVersions::Tls12Only;
        assert!(
            NetworkProfiles::default()
                .edited(1, vec![invalid], None)
                .is_err()
        );
        let mut blocked = custom();
        blocked.settings.protocols = transmog_core::connection_policy::AllowedHttpProtocols {
            http1: false,
            http2: false,
            http3: false,
        };
        assert!(
            NetworkProfiles::default()
                .edited(1, vec![blocked], Some("custom-no-h3"))
                .is_ok()
        );
        let mut catalog = NetworkProfiles::default();
        catalog.active_profile.settings.ech_grease = false;
        assert!(catalog.validate().is_err());
    }
}
