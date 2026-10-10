//! Non-secret authorization configuration, scoped to HTTPS host and port.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Behavior when no enabled host rule matches an actual certificate request.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientCertificateDefault {
    /// Attempt the handshake with an empty client certificate message.
    #[default]
    NoCertificate,
    /// Suspend the handshake for an explicit nonmodal user decision.
    Prompt,
}

/// A uniquely selected public identity. Private keys and passwords never belong here.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCertificateSelector {
    /// Stable provider identifier and exact certificate fingerprint, if pinned.
    pub identity: Option<String>,
    /// Optional exact subject attributes, for deliberate certificate renewal.
    #[serde(default)]
    pub subject: BTreeMap<String, String>,
    /// Optional exact issuer attributes. All supplied attributes must match.
    #[serde(default)]
    pub issuer: BTreeMap<String, String>,
}
impl ClientCertificateSelector {
    /// Rejects empty selectors and bounds accepted attribute names and values.
    pub fn valid(&self) -> bool {
        (self.identity.is_some() || !self.subject.is_empty() || !self.issuer.is_empty())
            && self
                .identity
                .as_ref()
                .is_none_or(|id| !id.is_empty() && id.len() <= 256)
            && [&self.subject, &self.issuer].into_iter().all(|attributes| {
                attributes.len() <= 7
                    && attributes.iter().all(|(name, value)| {
                        matches!(
                            name.as_str(),
                            "CN" | "O" | "OU" | "C" | "L" | "ST" | "emailAddress"
                        ) && !value.is_empty()
                            && value.len() <= 512
                            && !value.chars().any(char::is_control)
                    })
            })
    }
}

/// First enabled matching rule determines the complete action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClientCertificateAction {
    /// Continue without a certificate, even if another rule would select one.
    NoCertificate,
    /// Select exactly one compatible identity; missing or ambiguous means no certificate.
    Automatic {
        /// Exact fingerprint or explicit renewal attributes.
        selector: ClientCertificateSelector,
    },
    /// Wait for a user decision, including when the candidate list is empty.
    Prompt,
}

/// One ordered authorization for an exact HTTPS host and port.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCertificateRule {
    /// Stable row identity for explicit reordering.
    pub id: String,
    /// Disabled rows do not participate in selection.
    pub enabled: bool,
    /// Canonical host without scheme, path, query or wildcard; IPv6 has no brackets.
    pub host: String,
    /// Exact TLS destination port.
    pub port: u16,
    /// Complete action; automatic failure never falls through to another row.
    pub mode: ClientCertificateAction,
}

/// Advanced client certificate settings. Default behavior discloses no identity.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCertificatePolicy {
    /// Used only when no enabled host rule matches.
    #[serde(default)]
    pub default: ClientCertificateDefault,
    /// User-controlled order; the first enabled exact match wins.
    #[serde(default)]
    pub rules: Vec<ClientCertificateRule>,
}
impl ClientCertificatePolicy {
    /// Bounds saved settings and rejects invalid origin and selector shapes.
    pub fn valid(&self) -> bool {
        let mut ids = std::collections::BTreeSet::new();
        self.rules.len() <= 256
            && self.rules.iter().all(|rule| {
                !rule.id.is_empty()
                    && rule.id.len() <= 128
                    && ids.insert(&rule.id)
                    && rule.port != 0
                    && !rule.host.is_empty()
                    && rule.host.len() <= 253
                    && (rule.host.parse::<std::net::IpAddr>().is_ok()
                        || rule.host.split('.').all(|label| {
                            !label.is_empty()
                                && label.len() <= 63
                                && !label.starts_with('-')
                                && !label.ends_with('-')
                                && label
                                    .bytes()
                                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                        }))
                    && match &rule.mode {
                        ClientCertificateAction::Automatic { selector } => selector.valid(),
                        _ => true,
                    }
            })
    }
    /// Returns the first enabled exact host/port match, independently of its action.
    pub fn matching_rule(&self, host: &str, port: u16) -> Option<&ClientCertificateRule> {
        self.rules
            .iter()
            .find(|rule| rule.enabled && rule.port == port && rule.host.eq_ignore_ascii_case(host))
    }
}
