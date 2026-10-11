//! Public desktop certificate configuration and transient connection decisions.
use crate::{AppError, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use transmog_core::client_authentication::ClientCertificatePolicy;
use transmog_tls::{
    client_authentication::{ClientCredentialSource, PolicyClientAuthenticationResolver},
    client_prompts::{ClientCertificateAnswer, ClientCertificateBroker, certificate_name},
};

/// Public identity metadata; contains no signing provider or private material.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCertificateIdentity {
    /// Exact source identity used by automatic rules.
    pub id: String,
    /// SHA-256 certificate fingerprint.
    pub fingerprint: String,
    /// Public subject name.
    pub subject: String,
    /// Public issuer name.
    pub issuer: String,
    /// Expiration from the public certificate.
    pub valid_until: String,
}
/// One blocked connection group, with choices fetched only when inspected.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCertificatePending {
    /// Decimal broker identifier, preserving precision across IPC.
    pub id: String,
    /// Verified logical origin host.
    pub host: String,
    /// Logical HTTPS port.
    pub port: u16,
    /// Readable owner category.
    pub caller: String,
    /// Waiting physical connection count.
    pub connections: usize,
    /// Compatible identity count.
    pub choice_count: usize,
    /// Generic inventory/eligibility warning, when present.
    pub warning: Option<String>,
}
/// Snapshot consumed by the persistent blocked-connection indicator.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCertificateStatus {
    /// Whether this application has a supported certificate provider.
    pub supported: bool,
    /// Live blocked groups only.
    pub pending: Vec<ClientCertificatePending>,
}
/// Actual verified server request and public eligible choices.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCertificateRequestDetail {
    /// Still-live connection group.
    pub pending: ClientCertificatePending,
    /// Compatible public identities for this request.
    pub choices: Vec<ClientCertificateIdentity>,
    /// Negotiated protocol version.
    pub tls_version: String,
    /// Number of actual accepted issuer restrictions.
    pub accepted_issuers: usize,
    /// Actual signature algorithm identifiers.
    pub signature_algorithms: Vec<String>,
}
/// An explicit transient decision; saving host authorization is a separate action.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClientCertificateDecision {
    /// Send the exact selected compatible public identity.
    Certificate {
        /// Source identifier from this request's choices.
        identity: String,
    },
    /// Continue this same handshake with no certificate.
    NoCertificate,
    /// Close these blocked connections.
    Cancel,
}
/// Shared controller facade. The desktop retains the Windows monitor owner.
#[derive(Clone)]
pub struct ClientCertificateManager {
    source: Arc<dyn ClientCredentialSource>,
    resolver: Arc<PolicyClientAuthenticationResolver>,
    broker: ClientCertificateBroker,
}
impl ClientCertificateManager {
    /// Shares an existing transport controller without transferring native keys.
    #[must_use]
    pub fn new(
        source: Arc<dyn ClientCredentialSource>,
        resolver: Arc<PolicyClientAuthenticationResolver>,
        broker: ClientCertificateBroker,
    ) -> Self {
        Self {
            source,
            resolver,
            broker,
        }
    }
    /// Resolver installed on independently scoped proxy and Composer transports.
    #[must_use]
    pub fn resolver(&self) -> Arc<PolicyClientAuthenticationResolver> {
        self.resolver.clone()
    }
    pub(crate) fn replace_policy(&self, policy: ClientCertificatePolicy) -> Result<(), AppError> {
        self.resolver
            .replace_policy(policy)
            .map(|_| ())
            .map_err(|_| {
                error(
                    ErrorCategory::InvalidInput,
                    "Client certificate policy could not be applied",
                )
            })
    }
    /// Lists supported software identities after an explicit Settings refresh.
    /// # Errors
    /// Returns a generic unavailable/limit error, without native provider details.
    pub async fn identities(&self) -> Result<Vec<ClientCertificateIdentity>, AppError> {
        let credentials = tokio::time::timeout(Duration::from_secs(5), self.source.credentials())
            .await
            .map_err(|_| {
                error(
                    ErrorCategory::Unavailable,
                    "Client certificate inventory lookup timed out",
                )
            })?
            .map_err(|_| {
                error(
                    ErrorCategory::Unavailable,
                    "Windows client certificate inventory is unavailable",
                )
            })?;
        if credentials.len() > 1024 {
            return Err(error(
                ErrorCategory::Limit,
                "Client certificate inventory is too large",
            ));
        }
        tokio::task::spawn_blocking(move || {
            credentials
                .iter()
                .filter_map(|credential| {
                    let leaf = credential.chain.first()?;
                    Some(ClientCertificateIdentity {
                        id: credential.id.clone(),
                        fingerprint: credential.fingerprint()?,
                        subject: certificate_name(leaf.subject_name()),
                        issuer: certificate_name(leaf.issuer_name()),
                        valid_until: leaf.not_after().to_string(),
                    })
                })
                .collect()
        })
        .await
        .map_err(|_| {
            error(
                ErrorCategory::Unavailable,
                "Client certificate inventory is unavailable",
            )
        })
    }
    /// Reads all groups, or only one immutable Composer caller's groups for a viewer.
    #[must_use]
    pub fn status(&self, caller: Option<&str>) -> ClientCertificateStatus {
        ClientCertificateStatus {
            supported: true,
            pending: self
                .broker
                .pending()
                .iter()
                .filter(|pending| caller.is_none_or(|caller| caller == pending.context.caller))
                .map(summary)
                .collect(),
        }
    }
    /// Fetches a still-live request's eligible public choices.
    /// # Errors
    /// Rejects expired or invalid broker identifiers.
    pub fn request(&self, id: &str) -> Result<ClientCertificateRequestDetail, AppError> {
        let id = request_id(id)?;
        let pending = self
            .broker
            .pending()
            .into_iter()
            .find(|pending| pending.id == id)
            .ok_or_else(expired)?;
        Ok(ClientCertificateRequestDetail {
            pending: summary(&pending),
            choices: pending
                .choices
                .into_iter()
                .map(|choice| ClientCertificateIdentity {
                    id: choice.id,
                    fingerprint: choice.fingerprint,
                    subject: choice.subject,
                    issuer: choice.issuer,
                    valid_until: choice.valid_until,
                })
                .collect(),
            tls_version: match pending.request.tls_version {
                Some(boring::ssl::SslVersion::TLS1_2) => "TLS 1.2",
                Some(boring::ssl::SslVersion::TLS1_3) => "TLS 1.3",
                _ => "Unavailable",
            }
            .into(),
            accepted_issuers: pending.request.issuers.len(),
            signature_algorithms: pending
                .request
                .signature_algorithms
                .iter()
                .map(|id| format!("0x{id:04x}"))
                .collect(),
        })
    }
    /// Answers only the live broker entry; host or caller supplied by UI cannot grant access.
    /// # Errors
    /// Rejects stale identifiers and choices outside this actual server request.
    pub fn answer(&self, id: &str, decision: ClientCertificateDecision) -> Result<(), AppError> {
        let id = request_id(id)?;
        let answer = match decision {
            ClientCertificateDecision::Certificate { identity }
                if !identity.is_empty() && identity.len() <= 256 =>
            {
                ClientCertificateAnswer::Identity(identity)
            }
            ClientCertificateDecision::Certificate { .. } => {
                return Err(error(
                    ErrorCategory::InvalidInput,
                    "Choose an eligible certificate",
                ));
            }
            ClientCertificateDecision::NoCertificate => ClientCertificateAnswer::NoCertificate,
            ClientCertificateDecision::Cancel => ClientCertificateAnswer::Cancel,
        };
        self.broker
            .answer(id, answer)
            .then_some(())
            .ok_or_else(expired)
    }
}
fn summary(
    pending: &transmog_tls::client_prompts::PendingClientCertificateRequest,
) -> ClientCertificatePending {
    ClientCertificatePending {
        id: pending.id.to_string(),
        host: pending.context.host.clone(),
        port: pending.context.port,
        caller: if pending.context.caller.starts_with("composer:viewer-") {
            "Capture viewer Composer"
        } else if pending.context.caller.starts_with("composer:") {
            "Composer"
        } else {
            "Proxy client"
        }
        .into(),
        connections: pending.connections,
        choice_count: pending.choices.len(),
        warning: pending.warning.clone(),
    }
}
fn request_id(id: &str) -> Result<u64, AppError> {
    id.parse().map_err(|_| {
        error(
            ErrorCategory::InvalidInput,
            "Invalid certificate request identifier",
        )
    })
}
fn expired() -> AppError {
    error(
        ErrorCategory::Conflict,
        "This connection is no longer waiting, or the certificate choice is no longer eligible. It may have closed or timed out.",
    )
}
fn error(category: ErrorCategory, message: &str) -> AppError {
    AppError::new(category, message, false)
}
