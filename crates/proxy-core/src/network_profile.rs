//! Versioned origin transport recipes and their public capture provenance.
use crate::connection_policy::AllowedHttpProtocols;
use serde::{Deserialize, Serialize};

/// Initial qualified Chromium recipe identifier.
pub const CHROMIUM_154: &str = "chromium-154";
/// Browser build used for the initial `ClientHello` comparison.
pub const CHROMIUM_REFERENCE_VERSION: &str = "154.0.8037.99";

/// Versions offered on TCP TLS; QUIC always requires TLS 1.3.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsVersions {
    /// Offer TLS 1.2 and TLS 1.3.
    #[default]
    Tls12And13,
    /// Offer TLS 1.2 only.
    Tls12Only,
    /// Offer TLS 1.3 only.
    Tls13Only,
}

/// Reserved algorithm and extension offers, including independent signature GREASE.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsGrease {
    /// Ordinary GREASE and signature-algorithm GREASE.
    #[default]
    Chromium,
    /// Ordinary GREASE without a reserved signature algorithm.
    NoSignature,
    /// Disable both forms of GREASE.
    Disabled,
}

/// Bounded, typed diagnostic overrides for an origin transport recipe.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Independent bounded diagnostic choices.
pub struct NetworkProfileSettings {
    /// HTTP protocols allowed for this recipe.
    pub protocols: AllowedHttpProtocols,
    /// Share physical origin connections between requests.
    pub reuse_connections: bool,
    /// TCP TLS version offer.
    pub tls_versions: TlsVersions,
    /// Include Chromium's CBC and static-RSA TLS 1.2 suites.
    pub compatibility_ciphers: bool,
    /// Prefer hybrid post-quantum exchange ahead of classical groups.
    pub hybrid_key_exchange: bool,
    /// Reserved TLS offers.
    pub grease: TlsGrease,
    /// Randomize `ClientHello` extension order.
    pub randomize_extensions: bool,
    /// Send a dummy ECH extension; this does not encrypt the server name.
    pub ech_grease: bool,
    /// Accept Brotli-compressed server certificates.
    pub certificate_compression: bool,
    /// Negotiate HTTP/2 settings in TLS; Client Hint preferences remain with the caller.
    /// Older capture metadata omitted ALPS because it was not offered.
    #[serde(default)]
    pub http2_alps: bool,
}

impl Default for NetworkProfileSettings {
    fn default() -> Self {
        Self {
            protocols: AllowedHttpProtocols::default(),
            reuse_connections: true,
            tls_versions: TlsVersions::default(),
            compatibility_ciphers: true,
            hybrid_key_exchange: true,
            grease: TlsGrease::default(),
            randomize_extensions: true,
            ech_grease: true,
            certificate_compression: true,
            http2_alps: true,
        }
    }
}

impl NetworkProfileSettings {
    /// Validates combinations before persistence or transport construction.
    ///
    /// # Errors
    /// Returns an actionable field error for incompatible HTTP/3 and TLS choices.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.protocols.http3 && self.tls_versions == TlsVersions::Tls12Only {
            return Err("HTTP/3 requires TLS 1.3. Enable TLS 1.3 or turn off HTTP/3.");
        }
        Ok(())
    }
}

/// Immutable recipe attached to a physical origin connection, including its actual settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkProfileSnapshot {
    /// Stable preset or authored profile identifier.
    pub id: String,
    /// Authored display name at connection creation.
    pub name: String,
    /// Definition revision at connection creation.
    pub revision: u64,
    /// Immutable baseline identifier.
    pub base_profile: String,
    /// Fully resolved, effective diagnostic settings.
    pub settings: NetworkProfileSettings,
}

impl Default for NetworkProfileSnapshot {
    fn default() -> Self {
        Self {
            id: CHROMIUM_154.into(),
            name: "Chromium 154".into(),
            revision: 2,
            base_profile: CHROMIUM_154.into(),
            settings: NetworkProfileSettings::default(),
        }
    }
}

impl NetworkProfileSnapshot {
    /// Checks finite public provenance before accepting imported capture metadata.
    #[must_use]
    pub fn valid(&self) -> bool {
        !self.id.is_empty()
            && self.id.len() <= 80
            && !self.name.is_empty()
            && self.name.chars().count() <= 80
            && !self.base_profile.is_empty()
            && self.base_profile.len() <= 80
            && self.revision > 0
            && self.settings.validate().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_capture_recipe_does_not_claim_an_alps_offer() {
        let mut value = serde_json::to_value(NetworkProfileSnapshot::default()).unwrap();
        value["revision"] = 1.into();
        value["settings"]
            .as_object_mut()
            .unwrap()
            .remove("http2Alps");
        let historical: NetworkProfileSnapshot = serde_json::from_value(value).unwrap();
        assert!(!historical.settings.http2_alps);
        assert_eq!(historical.revision, 1);
        assert!(historical.valid());
        assert!(NetworkProfileSnapshot::default().settings.http2_alps);
    }
}
