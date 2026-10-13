//! Observable negotiation context across saved formats and certificate inspection.
use boring::{hash::MessageDigest, x509::X509Ref};
use std::{collections::BTreeMap, io::Read, path::Path, sync::Arc};
use transmog_app::{
    AppConfig, Application, BodyStoreConfig, SessionQueryInput, TraceImportRequest,
    TraceSaveFormat, TraceSaveOptions,
};
use transmog_capture::{
    CaptureLimits, CapturePassword, CaptureRecord, CaptureRecordKind, CaptureWriter,
};
use transmog_core::{
    PublicCertificate, TlsEvidence, VerificationResult,
    observe::ExchangeBoundary,
    performance::{PerformanceEvidence, TransportObservation, TransportOutcome},
};
use transmog_tls::{EndpointIdentity, LeafKeyAlgorithm, MlDsaVariant, ProxyCa};

const PASSWORD: &str = "TLS context regression password";

fn public(cert: &X509Ref) -> PublicCertificate {
    use std::fmt::Write as _;
    let mut sha256 = String::new();
    for byte in cert.digest(MessageDigest::sha256()).unwrap().iter() {
        write!(&mut sha256, "{byte:02x}").unwrap();
    }
    PublicCertificate {
        sha256,
        der: cert.to_der().unwrap().into(),
    }
}

fn evidence(state: u8) -> PerformanceEvidence {
    let ca = ProxyCa::generate("TLS context regression root", 2).unwrap();
    let identity = EndpointIdentity::parse("localhost").unwrap();
    let browser = ca
        .issue_with_algorithm(
            identity.clone(),
            1,
            LeafKeyAlgorithm::MlDsa(MlDsaVariant::MlDsa44),
        )
        .unwrap();
    let origin = ca.issue(identity, 1).unwrap();
    let resumed = state != 0;
    let tls12 = state == 2;
    let transports = ["client", "upstream"]
        .into_iter()
        .map(|leg| {
            let details = negotiation_details(leg, state);
            let (version, cipher) = version_cipher(state);
            TransportObservation {
                leg: leg.into(),
                connection_id: format!("{leg}-context"),
                outcome: TransportOutcome::Connected,
                tunnel_destination: (leg == "client").then(|| "localhost:443".into()),
                tls_version: Some(version.into()),
                cipher: Some(cipher.into()),
                tls_resumed: Some(resumed),
                tls: Some(TlsEvidence {
                    server_name: Some("localhost".into()),
                    server_chain: vec![
                        public(if leg == "client" && !tls12 {
                            &browser.certificate
                        } else {
                            &origin.certificate
                        }),
                        public(ca.certificate()),
                    ],
                    chain_source: Some(
                        if resumed {
                            "resumed-session"
                        } else {
                            "verification-path"
                        }
                        .into(),
                    ),
                    verification: Some(VerificationResult::Verified),
                    details,
                    ..Default::default()
                }),
                ..Default::default()
            }
        })
        .collect();
    PerformanceEvidence {
        transports,
        ..Default::default()
    }
}

fn version_cipher(state: u8) -> (&'static str, &'static str) {
    let tls12 = state == 2;
    let version = if tls12 { "TLSv1.2" } else { "TLSv1.3" };
    let cipher = if tls12 {
        "ECDHE-ECDSA-AES128-GCM-SHA256"
    } else {
        "TLS_AES_128_GCM_SHA256"
    };
    (version, cipher)
}

fn negotiation_details(leg: &str, state: u8) -> BTreeMap<String, String> {
    let resumed = state != 0;
    let tls12 = state == 2;
    let hybrid = !tls12 && (leg == "client" || resumed);
    let group = if hybrid { "X25519MLKEM768" } else { "X25519" };
    let mut details = BTreeMap::from([
        (
            "Session resumed".into(),
            if resumed { "Yes" } else { "No" }.into(),
        ),
        ("HelloRetryRequest".into(), "No".into()),
    ]);
    if tls12 {
        details.extend([
            (
                "Key exchange".into(),
                "Not performed (TLS 1.2 session resumed)".into(),
            ),
            ("Original session group".into(), "P-256".into()),
            ("Original session group ID".into(), "0x0017".into()),
        ]);
    } else {
        details.extend([
            (
                "Key exchange".into(),
                if hybrid {
                    "Hybrid post-quantum key exchange"
                } else {
                    "Classical key exchange"
                }
                .into(),
            ),
            ("Negotiated group".into(), group.into()),
            (
                "Negotiated group ID".into(),
                if hybrid { "0x11ec" } else { "0x001d" }.into(),
            ),
        ]);
    }
    if leg == "upstream" {
        details.insert(
            "Peer handshake signature".into(),
            if resumed {
                "Not sent (session resumed)"
            } else {
                "ecdsa_secp256r1_sha256"
            }
            .into(),
        );
        if resumed {
            details.insert(
                "Original peer handshake signature".into(),
                "ecdsa_secp256r1_sha256".into(),
            );
        }
    } else if !resumed {
        details.insert("Local handshake signature".into(), "mldsa44".into());
    }
    let (version, cipher) = version_cipher(state);
    details.insert("Negotiated TLS version".into(), version.into());
    details.insert("Negotiated cipher".into(), cipher.into());
    details
}

fn write_capture(path: &Path, samples: &[PerformanceEvidence]) {
    let mut writer = CaptureWriter::new(
        std::fs::File::create(path).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    let mut sequence = 0;
    let mut append = |kind| {
        writer
            .append(&CaptureRecord {
                sequence,
                exchange_id: 1,
                kind,
            })
            .unwrap();
        sequence += 1;
    };
    append(CaptureRecordKind::RequestHead {
        boundary: ExchangeBoundary::ClientRequest,
        method: "GET".into(),
        target: "https://localhost/context".into(),
        headers: vec![],
    });
    append(CaptureRecordKind::ResponseHead {
        boundary: ExchangeBoundary::ClientResponse,
        status: 200,
        headers: vec![],
    });
    for sample in samples {
        append(CaptureRecordKind::Performance(sample.clone()));
    }
    append(CaptureRecordKind::Completed);
    writer.seal().unwrap();
}

async fn open(path: &Path, cache: &Path, encrypted: bool) -> Application {
    let app = Application::new(AppConfig {
        body_store: Some(BodyStoreConfig::product_default(cache.to_owned())),
        ..Default::default()
    })
    .unwrap();
    app.import_trace(
        TraceImportRequest {
            path: path.to_owned(),
            password: encrypted.then(|| CapturePassword::new(PASSWORD.into())),
            operation_id: "TLS context".into(),
            max_file_bytes: u64::MAX,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    app
}

fn only_id(app: &Application) -> String {
    let rows = app.query_sessions(SessionQueryInput::default()).unwrap();
    assert_eq!(rows.sessions.len(), 1);
    rows.sessions[0].id.clone()
}

#[tokio::test]
async fn tls_facts_survive_native_and_both_saz_profiles_with_encryption() {
    let root = tempfile::tempdir().unwrap();
    for state in 0..3 {
        let expected = evidence(state);
        let source = root.path().join(format!("source-{state}.tmcap"));
        write_capture(&source, std::slice::from_ref(&expected));
        let app = open(
            &source,
            &root.path().join(format!("cache-source-{state}")),
            false,
        )
        .await;
        for format in [
            TraceSaveFormat::Native,
            TraceSaveFormat::SazStrict,
            TraceSaveFormat::SazExtended,
        ] {
            for encrypted in [false, true] {
                let extension = if format == TraceSaveFormat::Native {
                    "tmcap"
                } else {
                    "saz"
                };
                let saved = root
                    .path()
                    .join(format!("{state}-{format:?}-{encrypted}.{extension}"));
                app.save_traffic_trace(
                    saved.clone(),
                    TraceSaveOptions {
                        format,
                        password: encrypted.then(|| CapturePassword::new(PASSWORD.into())),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
                if extension == "saz" {
                    assert_saz_text(&saved, encrypted, &expected);
                }
                let reopened = open(
                    &saved,
                    &root
                        .path()
                        .join(format!("cache-{state}-{format:?}-{encrypted}")),
                    encrypted,
                )
                .await;
                let id = only_id(&reopened);
                assert_inspection(&reopened, &id, &expected, format, state);
            }
        }
    }
}

fn assert_inspection(
    reopened: &Application,
    id: &str,
    expected: &PerformanceEvidence,
    format: TraceSaveFormat,
    state: u8,
) {
    let inspection = reopened.inspect_certificates(id).unwrap();
    assert_eq!(
        inspection.default_connection_id,
        "upstream:upstream-context"
    );
    let snapshot = reopened.session_detail(id).unwrap();
    for original in &expected.transports {
        let connection = inspection
            .connections
            .iter()
            .find(|connection| {
                connection.id == format!("{}:{}", original.leg, original.connection_id)
            })
            .unwrap();
        for (label, value) in &original.tls.as_ref().unwrap().details {
            assert!(
                connection
                    .fields
                    .iter()
                    .chain(&connection.diagnostic_fields)
                    .any(|field| &field.label == label && &field.value == value),
                "{format:?} lost {label} for {}",
                original.leg
            );
        }
        if state == 2 {
            assert!(
                !connection
                    .fields
                    .iter()
                    .any(|field| field.label == "Negotiated group")
            );
        }
        let transport = snapshot
            .performance
            .transports
            .iter()
            .find(|transport| transport.leg == original.leg)
            .unwrap();
        assert_eq!(transport.tls_resumed, original.tls_resumed);
    }
    if format != TraceSaveFormat::SazStrict {
        assert!(
            inspection
                .certificates
                .iter()
                .all(|certificate| !certificate.der_base64.is_empty())
        );
        assert_eq!(
            inspection.certificates.len(),
            if state == 2 { 2 } else { 3 }
        );
    }
}

fn assert_saz_text(path: &Path, encrypted: bool, expected: &PerformanceEvidence) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut summaries = String::new();
    let mut metadata = String::new();
    for index in 0..archive.len() {
        let mut member = if encrypted {
            archive
                .by_index_decrypt(index, PASSWORD.as_bytes())
                .unwrap()
        } else {
            archive.by_index(index).unwrap()
        };
        let name = member.name().to_owned();
        if name.ends_with("_s.txt") {
            member.read_to_string(&mut summaries).unwrap();
        }
        if name.ends_with("_m.xml") {
            member.read_to_string(&mut metadata).unwrap();
        }
    }
    for transport in &expected.transports {
        for (label, value) in &transport.tls.as_ref().unwrap().details {
            assert!(
                summaries.contains(&format!("{label}: {value}")),
                "SAZ summary lost {label}"
            );
            assert!(metadata.contains(value), "SAZ metadata lost {label}");
        }
    }
}

#[tokio::test]
async fn strict_saz_primary_tls_metadata_survives_crowded_flags() {
    let root = tempfile::tempdir().unwrap();
    let expected = evidence(0);
    let source = root.path().join("source.tmcap");
    write_capture(&source, std::slice::from_ref(&expected));
    let app = open(&source, &root.path().join("cache-source"), false).await;
    let saved = root.path().join("source.saz");
    app.save_traffic_trace(
        saved.clone(),
        TraceSaveOptions {
            format: TraceSaveFormat::SazStrict,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for conventional in [true, false] {
        let crowded = root.path().join(format!("crowded-{conventional}.saz"));
        crowd_saz_flags(&saved, &crowded, conventional);
        let reopened = open(
            &crowded,
            &root.path().join(format!("cache-crowded-{conventional}")),
            false,
        )
        .await;
        let id = only_id(&reopened);
        assert_inspection(&reopened, &id, &expected, TraceSaveFormat::SazStrict, 0);
        let snapshot = reopened.session_detail(&id).unwrap();
        assert!(snapshot.performance.valid());
        for transport in &snapshot.performance.transports {
            assert_eq!(transport.tls_version.as_deref(), Some("TLSv1.3"));
            assert_eq!(transport.cipher.as_deref(), Some("TLS_AES_128_GCM_SHA256"));
            let tls = transport.tls.as_ref().unwrap();
            assert_eq!(tls.server_name.as_deref(), Some("localhost"));
            assert!(tls.details.len() <= 64);
        }
        // Imported annotations must not prevent saving a bounded native capture.
        reopened
            .save_traffic_trace(
                root.path().join(format!("crowded-{conventional}.tmcap")),
                TraceSaveOptions::default(),
            )
            .await
            .unwrap();
    }
}

fn crowd_saz_flags(source: &Path, destination: &Path, conventional: bool) {
    use std::{fmt::Write as _, io::Write as _};
    let mut archive = zip::ZipArchive::new(std::fs::File::open(source).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(destination).unwrap());
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).unwrap();
        let name = member.name().to_owned();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        if name.ends_with("_m.xml") {
            let mut xml = String::from_utf8(bytes).unwrap();
            let mut diagnostics = String::new();
            for leg in ["client", "server"] {
                for flag in 0..60 {
                    write!(
                        diagnostics,
                        "<SessionFlag N=\"https-{leg}-aaaa-{flag:03}\" V=\"diagnostic\"/>"
                    )
                    .unwrap();
                }
                if !conventional {
                    for (suffix, value) in [
                        ("protocol", "TLSv1.3"),
                        ("cipher", "TLS_AES_128_GCM_SHA256"),
                    ] {
                        let flag =
                            format!("<SessionFlag N=\"https-{leg}-{suffix}\" V=\"{value}\"/>");
                        xml = xml.replace(&flag, "");
                    }
                }
            }
            assert!(xml.contains("</SessionFlags>"));
            bytes = xml
                .replace("</SessionFlags>", &format!("{diagnostics}</SessionFlags>"))
                .into_bytes();
        }
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap();
}

#[tokio::test]
async fn certificate_inspection_prefers_complete_later_references() {
    let root = tempfile::tempdir().unwrap();
    let complete = evidence(0);
    let mut sparse = complete.clone();
    sparse
        .transports
        .retain(|transport| transport.leg == "upstream");
    for certificate in &mut sparse.transports[0].tls.as_mut().unwrap().server_chain {
        certificate.der = Arc::from([]);
    }
    let source = root.path().join("late.tmcap");
    // Complete references arrive later without rewriting the earlier server path.
    let mut client = complete.transports[0].clone();
    client.tls.as_mut().unwrap().server_chain = complete.transports[1]
        .tls
        .as_ref()
        .unwrap()
        .server_chain
        .clone();
    client.tls.as_mut().unwrap().details.insert(
        "Local handshake signature".into(),
        "ecdsa_secp256r1_sha256".into(),
    );
    let later = PerformanceEvidence {
        transports: vec![client],
        ..Default::default()
    };
    write_capture(&source, &[sparse, later]);
    let app = open(&source, &root.path().join("cache"), false).await;
    let inspection = app.inspect_certificates(&only_id(&app)).unwrap();
    assert_eq!(inspection.certificates.len(), 2);
    for certificate in &inspection.certificates {
        assert!(!certificate.der_base64.is_empty());
        assert!(certificate.pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(certificate.unavailable.is_empty());
    }
}

#[tokio::test]
async fn connection_selection_prefers_success_and_distinguishes_failed_attempts() {
    let root = tempfile::tempdir().unwrap();
    let mut sample = evidence(0);
    let mut failed = sample.transports[1].clone();
    failed.connection_id = "failed-attempt".into();
    failed.outcome = TransportOutcome::TlsFailed;
    sample.transports.insert(1, failed);
    for connected in [true, false] {
        if !connected {
            sample.transports[2].outcome = TransportOutcome::TcpFailed;
        }
        let source = root.path().join(format!("selection-{connected}.tmcap"));
        write_capture(&source, std::slice::from_ref(&sample));
        let app = open(
            &source,
            &root.path().join(format!("cache-{connected}")),
            false,
        )
        .await;
        let inspection = app.inspect_certificates(&only_id(&app)).unwrap();
        assert_eq!(
            inspection.default_connection_id,
            "upstream:upstream-context"
        );
        let titles = inspection
            .connections
            .iter()
            .map(|connection| &connection.title)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(titles.len(), 3);
        assert!(inspection.connections[1].title.contains("TLS failed"));
        assert!(inspection.connections[1].title.contains("failed-attempt"));
        assert!(inspection.connections[2].title.contains(if connected {
            "Connected"
        } else {
            "TCP failed"
        }));
    }
}
