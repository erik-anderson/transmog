//! Public certificate inspection shared by live traffic and saved trace windows.
use crate::{AppError, Application, ErrorCategory};
use base64::{Engine, engine::general_purpose::STANDARD};
use boring::x509::{X509, X509NameRef};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use transmog_core::{PublicCertificate, VerificationResult};
mod alps;
mod context;
pub use alps::AlpsContents;

/// A readable field derived from recorded public certificate bytes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateField {
    /// Field label.
    pub label: String,
    /// Original value, without display truncation.
    pub value: String,
    /// Visible projection of controls; the original remains available in `value`.
    pub display_value: String,
    /// Explains any differences between the displayed and original text.
    pub display_note: String,
}
/// One deduplicated public X.509 certificate.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateView {
    /// SHA-256 identity of the original DER.
    pub sha256: String,
    /// Subject or a clear indication that DER was not included.
    pub subject: String,
    /// Certificate properties derived from the original bytes.
    pub fields: Vec<CertificateField>,
    /// Public PEM, suitable for copying.
    pub pem: String,
    /// Original public DER for the shared Hex viewer.
    pub der_base64: String,
    /// Explanation when the capture omitted or damaged the public DER.
    pub unavailable: String,
}
/// Certificate paths and diagnostics for one physical connection.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateConnection {
    /// Stable physical connection identity.
    pub id: String,
    /// Identifies the browser-facing or server-facing leg.
    pub title: String,
    /// Ordered negotiation and recorded verification properties.
    pub fields: Vec<CertificateField>,
    /// Compact summary of recorded protocol, current group and cipher.
    pub summary: String,
    /// Connection identity, recorded trust provenance and configured policy.
    pub policy_fields: Vec<CertificateField>,
    /// Additional original metadata and conflicting recorded values.
    pub diagnostic_fields: Vec<CertificateField>,
    /// Decoded server ALPS and its original bytes, when negotiation was recorded.
    pub alps: Option<AlpsContents>,
    /// Server path, leaf first, referencing the shared certificate list.
    pub server_chain: Vec<String>,
    /// Client authentication path, if recorded.
    pub client_chain: Vec<String>,
    /// Recorded negotiation and verification failures.
    pub errors: Vec<String>,
}
/// Public TLS context belonging to the selected request's connections.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateInspection {
    /// Preferred current upstream connection, with failed attempts retained for inspection.
    pub default_connection_id: String,
    /// Connections corresponding to the request.
    pub connections: Vec<CertificateConnection>,
    /// Each public certificate appears once, including shared roots.
    pub certificates: Vec<CertificateView>,
}
#[allow(clippy::needless_pass_by_value)]
fn field(label: &str, value: impl ToString) -> CertificateField {
    let value = value.to_string();
    let display_value = context::display_text(&value);
    let value_note = context::display_note(&value);
    let label_note = context::display_note(label);
    CertificateField {
        label: context::display_text(label),
        display_note: if label_note.is_empty() {
            value_note
        } else {
            format!("Label: {label_note} {value_note}")
                .trim_end()
                .to_owned()
        },
        value,
        display_value,
    }
}
fn distinguished_name(name: &X509NameRef) -> String {
    name.entries()
        .map(|entry| {
            format!(
                "{}={}",
                entry.object().nid().short_name().unwrap_or("OID"),
                String::from_utf8_lossy(entry.data().as_slice())
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
fn certificate_view(certificate: &PublicCertificate) -> CertificateView {
    let mut view = CertificateView {
        sha256: certificate.sha256.clone(),
        subject: "Certificate bytes unavailable".into(),
        fields: vec![field("SHA-256", &certificate.sha256)],
        pem: String::new(),
        der_base64: String::new(),
        unavailable: String::new(),
    };
    if certificate.der.is_empty() {
        view.unavailable =
            "This capture contains a fingerprint without the public certificate bytes.".into();
        return view;
    }
    let Ok(parsed) = X509::from_der(&certificate.der) else {
        view.unavailable = "The recorded public certificate could not be parsed.".into();
        return view;
    };
    view.subject = distinguished_name(parsed.subject_name());
    view.fields.extend([
        field("Subject", &view.subject),
        field("Issuer", distinguished_name(parsed.issuer_name())),
        field("Valid from", parsed.not_before()),
        field("Valid until", parsed.not_after()),
    ]);
    view.fields.push(field(
        "Signature algorithm",
        parsed
            .signature_algorithm()
            .object()
            .nid()
            .long_name()
            .unwrap_or("Unknown"),
    ));
    if let Ok(key) = parsed.public_key() {
        let algorithm = match key.id() {
            boring::pkey::Id::RSA => "RSA",
            boring::pkey::Id::EC => "EC",
            boring::pkey::Id::ED25519 => "Ed25519",
            _ => "Other",
        };
        let description = transmog_tls::MlDsaVariant::for_key(&key).map_or_else(
            || format!("{algorithm} · {} bits", key.bits()),
            |variant| match variant {
                transmog_tls::MlDsaVariant::MlDsa44 => "ML-DSA-44".into(),
                transmog_tls::MlDsaVariant::MlDsa65 => "ML-DSA-65".into(),
                transmog_tls::MlDsaVariant::MlDsa87 => "ML-DSA-87".into(),
            },
        );
        view.fields.push(field("Public key", description));
    }
    if let Ok(serial) = parsed
        .serial_number()
        .to_bn()
        .and_then(|number| number.to_hex_str())
    {
        view.fields.push(field("Serial number", serial));
    }
    if let Some(names) = parsed.subject_alt_names() {
        let names = names
            .iter()
            .filter_map(|name| {
                name.dnsname().map(str::to_owned).or_else(|| {
                    name.ipaddress().map(|bytes| match bytes.len() {
                        4 => {
                            std::net::IpAddr::from(<[u8; 4]>::try_from(bytes).expect("four bytes"))
                                .to_string()
                        }
                        16 => std::net::IpAddr::from(
                            <[u8; 16]>::try_from(bytes).expect("sixteen bytes"),
                        )
                        .to_string(),
                        _ => format!("IP: {bytes:?}"),
                    })
                })
            })
            .collect::<Vec<_>>();
        view.fields
            .push(field("Subject alternative names", names.join("\n")));
    }
    view.pem = parsed
        .to_pem()
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    view.der_base64 = STANDARD.encode(&certificate.der);
    view
}
impl Application {
    /// Reads public certificate paths and recorded negotiation diagnostics.
    /// Does not perform new verification against this computer's current trust.
    /// # Errors
    /// Returns a missing/evicted selection or invalid certificate evidence error.
    #[allow(clippy::too_many_lines)]
    pub fn inspect_certificates(&self, id: &str) -> Result<CertificateInspection, AppError> {
        let detail = self.session_detail(id)?;
        let mut certificates = BTreeMap::new();
        let mut connections = Vec::new();
        for transport in &detail.performance.transports {
            let mut fields = vec![
                field("Connection", &transport.connection_id),
                field("Outcome", context::outcome_label(transport.outcome)),
            ];
            if detail.terminal_state == "failed" {
                fields.push(field("Request result", &detail.terminal));
            }
            if let Some(peer) = &transport.peer {
                fields.push(field("Peer", peer));
            }
            let mut connection = CertificateConnection {
                id: format!("{}:{}", transport.leg, transport.connection_id),
                title: if transport.leg == "client" {
                    "Client → Transmog"
                } else {
                    "Transmog → server"
                }
                .into(),
                fields,
                summary: String::new(),
                policy_fields: Vec::new(),
                diagnostic_fields: Vec::new(),
                alps: None,
                server_chain: Vec::new(),
                client_chain: Vec::new(),
                errors: Vec::new(),
            };
            if let Some(tls) = &transport.tls {
                if tls.trust_source.as_deref()
                    == Some("Chromium NetLog (original browser verification)")
                {
                    connection.title = "Browser → server".into();
                } else if tls.details.keys().any(|key| {
                    key.starts_with("HAR ") || key.to_ascii_lowercase().starts_with("https-")
                }) {
                    connection.title = if transport.leg == "client" {
                        "Recorded client connection"
                    } else {
                        "Recorded server connection"
                    }
                    .into();
                }
                if !tls.valid() {
                    return Err(AppError::new(
                        ErrorCategory::InvalidInput,
                        "The saved TLS evidence exceeds its limits",
                        false,
                    ));
                }
                for (label, value) in [
                    ("Server name", &tls.server_name),
                    ("Chain source", &tls.chain_source),
                    ("Recorded trust source", &tls.trust_source),
                ] {
                    if let Some(value) = value {
                        connection.policy_fields.push(field(label, value));
                    }
                }
                if let Some(generation) = tls.trust_generation {
                    connection
                        .policy_fields
                        .push(field("Recorded trust generation", generation));
                }
                if let Some(profile) = &tls.network_profile {
                    connection
                        .policy_fields
                        .push(field("Network profile", &profile.name));
                    connection
                        .policy_fields
                        .push(field("Profile revision", profile.revision));
                    connection
                        .policy_fields
                        .push(field("Profile baseline", &profile.base_profile));
                    connection.policy_fields.push(field(
                        "Effective network settings",
                        serde_json::to_string(&profile.settings).unwrap_or_default(),
                    ));
                }
                let verification = match &tls.verification {
                    Some(VerificationResult::Verified) => {
                        "Verified by the original connection".into()
                    }
                    Some(VerificationResult::Rejected { error_code, depth }) => format!(
                        "Rejected (code {error_code}, depth {})",
                        depth.map_or_else(|| "unavailable".into(), |depth| depth.to_string())
                    ),
                    None => "Not recorded or verification was not reached".into(),
                };
                connection
                    .fields
                    .push(field("Recorded verification", verification));
                for (label, values) in [
                    ("TLS version bounds", &tls.offered_versions),
                    ("Available cipher suites", &tls.offered_ciphers),
                    ("Offered ALPN", &tls.offered_alpn),
                ] {
                    if !values.is_empty() {
                        connection
                            .policy_fields
                            .push(field(label, values.join(", ")));
                    }
                }
                connection.errors = tls
                    .errors
                    .iter()
                    .map(|error| context::display_text(error))
                    .collect();
                for (chain, references) in [
                    (&tls.server_chain, &mut connection.server_chain),
                    (&tls.client_chain, &mut connection.client_chain),
                ] {
                    let mut seen = BTreeSet::new();
                    for certificate in chain {
                        if seen.insert(&certificate.sha256) {
                            references.push(certificate.sha256.clone());
                        }
                        certificates
                            .entry(certificate.sha256.clone())
                            .and_modify(|view: &mut CertificateView| {
                                if view.der_base64.is_empty() && !certificate.der.is_empty() {
                                    let complete = certificate_view(certificate);
                                    if !complete.der_base64.is_empty() {
                                        *view = complete;
                                    }
                                }
                            })
                            .or_insert_with(|| certificate_view(certificate));
                    }
                }
            }
            context::organize(&mut connection, transport);
            connections.push(connection);
        }
        let preferred = detail
            .performance
            .transports
            .iter()
            .rev()
            .find(|transport| {
                transport.leg == "upstream"
                    && transport.outcome == transmog_core::performance::TransportOutcome::Connected
            })
            .or_else(|| {
                detail
                    .performance
                    .transports
                    .iter()
                    .rev()
                    .find(|transport| transport.leg == "upstream")
            });
        let default_connection_id = preferred.map_or_else(
            || {
                connections
                    .first()
                    .map_or_else(|| "unavailable".into(), |connection| connection.id.clone())
            },
            |transport| format!("{}:{}", transport.leg, transport.connection_id),
        );
        for (connection, transport) in connections.iter_mut().zip(&detail.performance.transports) {
            let outcome = context::outcome_label(transport.outcome);
            connection.title = format!(
                "{} · {outcome} · {}",
                connection.title,
                context::display_text(&transport.connection_id)
            );
        }
        if connections.is_empty() {
            connections.push(CertificateConnection {
                id: "unavailable".into(),
                title: "TLS context unavailable".into(),
                fields: vec![field("Request result", detail.terminal)],
                summary: "TLS negotiation unavailable".into(),
                policy_fields: vec![],
                diagnostic_fields: vec![],
                alps: None,
                server_chain: vec![],
                client_chain: vec![],
                errors: vec![],
            });
        }
        Ok(CertificateInspection {
            default_connection_id,
            connections,
            certificates: certificates.into_values().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppConfig, BodyStoreConfig, TraceImportRequest, TraceSaveFormat, TraceSaveOptions,
    };
    use std::sync::Arc;
    use transmog_capture::{CaptureLimits, CaptureRecord, CaptureRecordKind, CaptureWriter};
    use transmog_core::{
        TlsEvidence,
        performance::{PerformanceEvidence, TransportObservation, TransportOutcome},
    };
    use transmog_tls::{EndpointIdentity, ProxyCa};

    fn configured_client_chain(
        ca: &ProxyCa,
        leaf: &transmog_tls::IssuedLeaf,
    ) -> Vec<PublicCertificate> {
        let trust = transmog_tls::PemTrustSource::from_pem(
            &ca.certificate_pem().unwrap(),
            "recorded client chain",
        )
        .unwrap();
        let tls = transmog_tls::UpstreamTlsContextFactory::new(
            Arc::new(transmog_tls::TrustSnapshot::load(&trust, 1).unwrap()),
            transmog_tls::UpstreamTlsPolicy::default(),
        )
        .with_client_authentication(
            leaf.certificate.clone(),
            leaf.private_key.clone(),
            vec![ca.certificate().clone()],
        );
        let context = tls.hyper_connector().unwrap();
        let capture = tls.capture(None, vec![]);
        capture.completed(&boring::ssl::Ssl::new(context.context()).unwrap(), false);
        capture.snapshot().client_chain
    }
    fn fixture(path: &std::path::Path) -> (Vec<PublicCertificate>, Vec<u8>) {
        let ca = ProxyCa::generate("Recorded certificate test root", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let chain = [&leaf.certificate, ca.certificate()]
            .into_iter()
            .map(|certificate| {
                let der = certificate.to_der().unwrap();
                let digest = certificate
                    .digest(boring::hash::MessageDigest::sha256())
                    .unwrap();
                let sha256 = digest.iter().fold(String::new(), |mut output, byte| {
                    use std::fmt::Write as _;
                    let _ = write!(output, "{byte:02x}");
                    output
                });
                PublicCertificate {
                    sha256,
                    der: der.into(),
                }
            })
            .collect::<Vec<_>>();
        let client_chain = configured_client_chain(&ca, &leaf);
        assert_eq!(client_chain, chain);
        let mut writer = CaptureWriter::new(
            std::fs::File::create(path).unwrap(),
            CaptureLimits::default(),
        )
        .unwrap();
        let evidence = PerformanceEvidence {
            transports: vec![TransportObservation {
                leg: "upstream".into(),
                connection_id: "recorded-h2".into(),
                outcome: TransportOutcome::TlsFailed,
                tls: Some(TlsEvidence {
                    server_name: Some("localhost".into()),
                    server_chain: chain.clone(),
                    client_chain,
                    verification: Some(VerificationResult::Rejected {
                        error_code: 20,
                        depth: Some(1),
                    }),
                    offered_versions: vec!["TLSv1.3".into()],
                    offered_ciphers: vec!["TLS_AES_128_GCM_SHA256".into()],
                    errors: vec![
                        "Certificate issuer was not trusted by the recorded snapshot".into(),
                    ],
                    ..TlsEvidence::default()
                }),
                ..TransportObservation::default()
            }],
            ..PerformanceEvidence::default()
        };
        for id in [1, 2] {
            let kinds = [
                CaptureRecordKind::RequestHead {
                    boundary: transmog_core::observe::ExchangeBoundary::ClientRequest,
                    method: "GET".into(),
                    target: "https://localhost/certificate".into(),
                    headers: vec![],
                },
                CaptureRecordKind::ResponseHead {
                    boundary: transmog_core::observe::ExchangeBoundary::ClientResponse,
                    status: 502,
                    headers: vec![],
                },
                CaptureRecordKind::Performance(evidence.clone()),
                CaptureRecordKind::Failed {
                    category: "Upstream".into(),
                    message: "Recorded TLS verification failure".into(),
                },
            ];
            for (sequence, kind) in kinds.into_iter().enumerate() {
                writer
                    .append(&CaptureRecord {
                        sequence: sequence as u64,
                        exchange_id: id,
                        kind,
                    })
                    .unwrap();
            }
        }
        writer.seal().unwrap();
        drop(writer);
        (chain, ca.private_key_pem_pkcs8().unwrap())
    }
    async fn open(path: std::path::PathBuf, cache: std::path::PathBuf) -> Application {
        let app = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(cache)),
            ..AppConfig::default()
        })
        .unwrap();
        app.import_trace(
            TraceImportRequest {
                password: None,
                path,
                operation_id: "certificates".into(),
                max_file_bytes: u64::MAX,
            },
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        app
    }
    #[tokio::test]
    async fn certificate_paths_failures_and_deduplication_survive_native_saz_and_har() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original.tmcap");
        let (chain, private_key) = fixture(&path);
        let recovered = transmog_capture::recover(
            std::fs::File::open(&path).unwrap(),
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(recovered.records.iter().filter(|record| matches!(&record.kind,CaptureRecordKind::Unknown { kind,.. } if kind == "public-certificate")).count(), 2);
        let paths = recovered
            .records
            .iter()
            .filter_map(|record| match &record.kind {
                CaptureRecordKind::Performance(performance) => {
                    Some(&performance.transports[0].tls.as_ref().unwrap().server_chain)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(Arc::ptr_eq(&paths[0][1].der, &paths[1][1].der));
        let app = open(path, root.path().join("cache-original")).await;
        for (index, format, extension) in [
            (0, TraceSaveFormat::Native, "tmcap"),
            (1, TraceSaveFormat::SazExtended, "saz"),
            (2, TraceSaveFormat::Har, "har"),
        ] {
            let saved = root.path().join(format!("saved-{index}.{extension}"));
            app.save_traffic_trace(
                saved.clone(),
                TraceSaveOptions {
                    format,
                    ..TraceSaveOptions::default()
                },
            )
            .await
            .unwrap();
            let raw = std::fs::read(&saved).unwrap();
            assert!(
                !raw.windows(private_key.len())
                    .any(|window| window == private_key)
            );
            let reopened = open(saved, root.path().join(format!("cache-{index}"))).await;
            let rows = reopened
                .query_sessions(crate::SessionQueryInput::default())
                .unwrap();
            assert_eq!(rows.sessions.len(), 2);
            let inspection = reopened.inspect_certificates(&rows.sessions[0].id).unwrap();
            assert_eq!(inspection.certificates.len(), 2);
            let connection = inspection
                .connections
                .iter()
                .find(|connection| connection.id.ends_with("recorded-h2"))
                .unwrap();
            assert_eq!(
                connection.server_chain,
                chain
                    .iter()
                    .map(|certificate| certificate.sha256.clone())
                    .collect::<Vec<_>>()
            );
            assert_eq!(connection.client_chain, connection.server_chain);
            assert!(
                connection
                    .fields
                    .iter()
                    .any(|field| field.label == "Key exchange" && field.value == "Unavailable")
            );
            assert!(connection.errors[0].contains("not trusted"));
            assert!(
                connection
                    .fields
                    .iter()
                    .any(|field| field.value.contains("Rejected (code 20"))
            );
            let leaf = inspection
                .certificates
                .iter()
                .find(|certificate| certificate.sha256 == chain[0].sha256)
                .unwrap();
            assert!(leaf.subject.contains("localhost"));
            assert!(leaf.pem.starts_with("-----BEGIN CERTIFICATE-----"));
            assert_eq!(
                STANDARD.decode(&leaf.der_base64).unwrap(),
                chain[0].der.as_ref()
            );
            assert!(
                leaf.fields
                    .iter()
                    .any(|field| field.label == "Subject alternative names"
                        && field.value == "localhost")
            );
        }
    }
    #[test]
    fn ml_dsa_public_keys_have_parameter_set_names() {
        let ca = ProxyCa::generate("ML-DSA viewer root", 2).unwrap();
        for (variant, expected) in [
            (transmog_tls::MlDsaVariant::MlDsa44, "ML-DSA-44"),
            (transmog_tls::MlDsaVariant::MlDsa65, "ML-DSA-65"),
            (transmog_tls::MlDsaVariant::MlDsa87, "ML-DSA-87"),
        ] {
            let leaf = ca
                .issue_with_algorithm(
                    EndpointIdentity::parse("localhost").unwrap(),
                    1,
                    transmog_tls::LeafKeyAlgorithm::MlDsa(variant),
                )
                .unwrap();
            let certificate = PublicCertificate {
                der: leaf.certificate.to_der().unwrap().into(),
                ..PublicCertificate::default()
            };
            let view = certificate_view(&certificate);
            assert!(
                view.fields
                    .iter()
                    .any(|field| field.label == "Public key" && field.value == expected)
            );
        }
    }

    #[test]
    fn public_certificate_definitions_reject_private_keys_trailing_data_and_wrong_digest() {
        let root = tempfile::tempdir().unwrap();
        let (chain, key) = fixture(&root.path().join("certificates.tmcap"));
        for der in [key, {
            let mut der = chain[0].der.to_vec();
            der.extend_from_slice(b"trailing secret");
            der
        }] {
            let digest = boring::hash::hash(boring::hash::MessageDigest::sha256(), &der).unwrap();
            let sha256 = digest.iter().fold(String::new(), |mut output, byte| {
                use std::fmt::Write as _;
                let _ = write!(output, "{byte:02x}");
                output
            });
            assert!(
                transmog_capture::CertificateDefinition { sha256, der }
                    .validate()
                    .is_err()
            );
        }
        assert!(
            transmog_capture::CertificateDefinition {
                sha256: "0".repeat(64),
                der: chain[0].der.to_vec()
            }
            .validate()
            .is_err()
        );
    }
}
