//! SAZ display projections and transport bounds across the public application APIs.
use std::{collections::BTreeMap, io::Read, path::Path, sync::Arc};
use transmog_app::{
    AppConfig, Application, BodyStoreConfig, ErrorCategory, SessionQueryInput, TraceImportRequest,
    TraceSaveFormat, TraceSaveOptions,
};
use transmog_capture::{
    CaptureLimits, CapturePassword, CaptureRecord, CaptureRecordKind, CaptureWriter,
};
use transmog_core::{
    TlsEvidence,
    observe::ExchangeBoundary,
    performance::{
        MAX_TUNNEL_DESTINATION_BYTES, PerformanceEvidence, TransportObservation, TransportOutcome,
    },
};

const PASSWORD: &str = "SAZ metadata regression password";

fn write_capture(path: &Path, group: &str) {
    let mut writer = CaptureWriter::new(
        std::fs::File::create(path).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    let kinds = [
        CaptureRecordKind::RequestHead {
            boundary: ExchangeBoundary::ClientRequest,
            method: "GET".into(),
            target: "https://localhost/metadata".into(),
            headers: vec![],
        },
        CaptureRecordKind::ResponseHead {
            boundary: ExchangeBoundary::ClientResponse,
            status: 200,
            headers: vec![],
        },
        CaptureRecordKind::Performance(PerformanceEvidence {
            transports: vec![TransportObservation {
                leg: "client".into(),
                connection_id: "browser".into(),
                outcome: TransportOutcome::Connected,
                tunnel_destination: Some("localhost:443".into()),
                tls_version: Some("TLSv1.3".into()),
                tls: Some(TlsEvidence {
                    server_name: Some("literal\\u{0000}.test".into()),
                    details: BTreeMap::from([
                        ("Negotiated group".into(), group.into()),
                        ("Negotiated group ID".into(), "0x11ec".into()),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }),
        CaptureRecordKind::Completed,
    ];
    for (sequence, kind) in kinds.into_iter().enumerate() {
        writer
            .append(&CaptureRecord {
                sequence: sequence as u64,
                exchange_id: 1,
                kind,
            })
            .unwrap();
    }
    writer.seal().unwrap();
}

fn configured(cache: &Path) -> Application {
    Application::new(AppConfig {
        body_store: Some(BodyStoreConfig::product_default(cache.to_owned())),
        ..Default::default()
    })
    .unwrap()
}

async fn import(
    app: &Application,
    path: &Path,
    encrypted: bool,
) -> Result<(), transmog_app::AppError> {
    app.import_trace(
        TraceImportRequest {
            path: path.to_owned(),
            password: encrypted.then(|| CapturePassword::new(PASSWORD.into())),
            operation_id: "SAZ metadata".into(),
            max_file_bytes: u64::MAX,
        },
        Arc::new(|_| {}),
    )
    .await
    .map(|_| ())
}

fn only_id(app: &Application) -> String {
    let rows = app.query_sessions(SessionQueryInput::default()).unwrap();
    assert_eq!(rows.sessions.len(), 1);
    rows.sessions[0].id.clone()
}

#[tokio::test]
async fn xml_forbidden_characters_export_and_round_trip_without_mutating_originals() {
    let root = tempfile::tempdir().unwrap();
    let groups = [
        "X25519\\u{0000}\0MLKEM768\u{ffff}".to_owned(),
        "\0".repeat(4096),
        "literal\\u{0000}".to_owned(),
    ];
    for (case, group) in groups.iter().enumerate() {
        let native = root.path().join(format!("source-{case}.tmcap"));
        write_capture(&native, group);
        let app = configured(&root.path().join(format!("cache-source-{case}")));
        import(&app, &native, false).await.unwrap();
        for (format, encrypted) in [
            (TraceSaveFormat::Native, false),
            (TraceSaveFormat::SazStrict, false),
            (TraceSaveFormat::SazStrict, true),
            (TraceSaveFormat::SazExtended, false),
            (TraceSaveFormat::SazExtended, true),
        ] {
            let extension = if format == TraceSaveFormat::Native {
                "tmcap"
            } else {
                "saz"
            };
            let saved = root
                .path()
                .join(format!("{case}-{format:?}-{encrypted}.{extension}"));
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
                assert_xml_projection(&saved, encrypted, case != 2);
            }
            let reopened = configured(
                &root
                    .path()
                    .join(format!("cache-{case}-{format:?}-{encrypted}")),
            );
            import(&reopened, &saved, encrypted).await.unwrap();
            assert_original_text(&reopened, group);
        }
        assert_original_text(&app, group);
    }
}

fn assert_original_text(app: &Application, group: &str) {
    let id = only_id(app);
    let snapshot = app.session_detail(&id).unwrap();
    assert!(snapshot.performance.valid());
    let tls = snapshot.performance.transports[0].tls.as_ref().unwrap();
    assert_eq!(tls.details["Negotiated group"], group);
    assert_eq!(tls.server_name.as_deref(), Some("literal\\u{0000}.test"));
    let inspected = app.inspect_certificates(&id).unwrap();
    assert!(
        inspected.connections[0]
            .fields
            .iter()
            .any(|field| field.label == "Negotiated group" && field.value == group)
    );
}

fn assert_xml_projection(path: &Path, encrypted: bool, projected: bool) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut xml_count = 0;
    for index in 0..archive.len() {
        let mut member = if encrypted {
            archive
                .by_index_decrypt(index, PASSWORD.as_bytes())
                .unwrap()
        } else {
            archive.by_index(index).unwrap()
        };
        if member.name().ends_with("_m.xml") {
            xml_count += 1;
            let mut xml = String::new();
            member.read_to_string(&mut xml).unwrap();
            assert!(!xml.contains('\0') && !xml.contains('\u{ffff}'));
            assert_eq!(
                xml.contains("TransmogXmlCharacterProjection=\"unicode-escapes\""),
                projected
            );
            assert_eq!(
                xml.contains("N=\"x-transmog-xml-character-projection\""),
                projected
            );
            if projected {
                assert!(xml.contains("\\u{0000}"));
            }
        }
        if member.name().ends_with("_s.txt") {
            let mut text = String::new();
            member.read_to_string(&mut text).unwrap();
            assert!(!text.contains('\0') && !text.contains('\u{ffff}'));
        }
    }
    assert!(xml_count > 0);
}

#[tokio::test]
async fn connect_destinations_enforce_the_shared_byte_bound_before_publishing() {
    let root = tempfile::tempdir().unwrap();
    let native = root.path().join("source.tmcap");
    write_capture(&native, "X25519MLKEM768");
    let source = configured(&root.path().join("cache-source"));
    import(&source, &native, false).await.unwrap();
    let saved = root.path().join("source.saz");
    source
        .save_traffic_trace(
            saved.clone(),
            TraceSaveOptions {
                format: TraceSaveFormat::SazStrict,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for (case, destination) in [
        format!("{}:443", "a".repeat(1020)),
        format!("{}:443", "a".repeat(1021)),
        format!("{}:443", "é".repeat(510)),
        format!("{}:443", "é".repeat(511)),
    ]
    .iter()
    .enumerate()
    {
        let altered = root.path().join(format!("destination-{case}.saz"));
        rewrite_destination(&saved, &altered, destination);
        let app = configured(&root.path().join(format!("cache-destination-{case}")));
        if destination.len() > MAX_TUNNEL_DESTINATION_BYTES {
            import(&app, &saved, false).await.unwrap();
            let original_id = only_id(&app);
            let error = import(&app, &altered, false).await.unwrap_err();
            assert_eq!(error.category, ErrorCategory::InvalidInput);
            assert_eq!(
                error.message,
                "The saved CONNECT destination exceeds its 1,024-byte limit"
            );
            assert!(!error.retryable);
            assert_eq!(only_id(&app), original_id);
        } else {
            import(&app, &altered, false).await.unwrap();
            let snapshot = app.session_detail(&only_id(&app)).unwrap();
            assert!(snapshot.performance.valid());
            assert_eq!(
                snapshot.performance.transports[0]
                    .tunnel_destination
                    .as_deref(),
                Some(destination.as_str())
            );
        }
        app.save_traffic_trace(
            root.path().join(format!("destination-{case}.tmcap")),
            TraceSaveOptions::default(),
        )
        .await
        .unwrap();
    }
}

fn rewrite_destination(source: &Path, destination: &Path, authority: &str) {
    use std::io::Write as _;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(source).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(destination).unwrap());
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).unwrap();
        let name = member.name().to_owned();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        if name.ends_with("_m.xml") {
            bytes = String::from_utf8(bytes)
                .unwrap()
                .replace(
                    "N=\"x-transmog-connect-destination\" V=\"localhost:443\"",
                    &format!("N=\"x-transmog-connect-destination\" V=\"{authority}\""),
                )
                .into_bytes();
        }
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap();
}
