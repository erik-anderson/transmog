use super::*;
use std::{fs::File, path::Path};
use transmog_capture::{
    CaptureLimits, CaptureRecord as Record, CaptureRecordKind as Kind, CaptureWriter,
    CapturedHeader,
};
use transmog_core::observe::ExchangeBoundary as Boundary;
const BASE: u64 = 1_800_000_000_000;
fn application(path: &Path) -> Application {
    Application::new(crate::AppConfig {
        body_store: Some(crate::BodyStoreConfig::product_default(path.join("cache"))),
        ..crate::AppConfig::default()
    })
    .unwrap()
}
fn fixture(path: &Path) -> IncidentMarker {
    let profile=SupportProfile::parse(br#"{"schemaVersion":1,"id":"site-issue","revision":1,"title":"Checkout failure","steps":["Open website","Reproduce checkout"],"relevantHosts":[{"host":"example.test","includeSubdomains":true}],"recording":{"redactSensitiveHeaders":true}}"#).unwrap();
    let mut marker = IncidentMarker::new(
        transmog_capture::support::new_id().unwrap(),
        Some(40_000),
        transmog_capture::support::MarkerOrigin::Cli,
        "Checkout failed <script>never execute</script>".into(),
    )
    .unwrap();
    marker.unix_millis = BASE;
    let mut writer =
        CaptureWriter::new(File::create(path).unwrap(), CaptureLimits::default()).unwrap();
    writer.append(&Record{sequence:0,exchange_id:0,kind:Kind::Unknown{kind:"trace-metadata".into(),payload:serde_json::json!({"captureRunId":marker.run_id,"application":"Transmog CLI","version":"fixture","supportCapture":{"version":1,"profileDigest":profile.digest().unwrap(),"profile":profile,"expectedBehavior":"Checkout succeeds","actualBehavior":"Error shown","clientVersionEntered":"Customer-entered Browser 123"}})}}).unwrap();
    for (id, url, delta) in [
        (1, "https://example.test/page", -40_000),
        (2, "https://example.test/api", -2_000),
        (3, "https://cdn.example.test/style.css", 2_000),
        (4, "https://other.test/metrics", 5_000),
    ] {
        let mut sequence = 0;
        let mut emit = |kind| {
            sequence += 1;
            writer
                .append(&Record {
                    sequence,
                    exchange_id: id,
                    kind,
                })
                .unwrap();
        };
        let millis = BASE.saturating_add_signed(delta);
        emit(Kind::ExchangeStarted {
            client_addr: "127.0.0.1:1".into(),
            client_identity: transmog_core::ClientIdentity::LocalUnknown,
            listener_addr: "127.0.0.1:2".into(),
            authority: "example.test:443".into(),
            started_unix_nanos: u128::from(millis) * 1_000_000,
        });
        emit(Kind::RequestHead {
            boundary: Boundary::ClientRequest,
            method: "GET".into(),
            target: url.into(),
            headers: vec![
                CapturedHeader {
                    name: b"User-Agent".to_vec(),
                    value: Some(b"Observed agent fixture".to_vec()),
                    original_value_bytes: None,
                },
                CapturedHeader {
                    name: b"Cookie".to_vec(),
                    value: Some(b"private-cookie".to_vec()),
                    original_value_bytes: None,
                },
            ],
        });
        emit(Kind::ResponseHead {
            boundary: Boundary::ClientResponse,
            status: 200,
            headers: vec![CapturedHeader {
                name: b"Content-Type".to_vec(),
                value: Some(b"text/plain".to_vec()),
                original_value_bytes: None,
            }],
        });
        emit(Kind::BodySegment {
            boundary: Boundary::ClientResponse,
            byte_count: 7,
            bytes: Some(b"private".to_vec()),
            truncated: false,
        });
        if id == 2 {
            emit(Kind::StreamState(transmog_core::stream::StreamState::now(
                transmog_core::stream::StreamProtocol::Sse,
                transmog_core::stream::StreamStatus::Open,
                None,
            )));
            emit(Kind::StreamMessage(
                transmog_core::stream::StreamMessage::new(
                    "server",
                    "event",
                    bytes::Bytes::from_static(b"private stream content"),
                ),
            ));
        }
        emit(Kind::Completed);
    }
    writer.append(&marker.record().unwrap()).unwrap();
    writer.seal().unwrap();
    marker
}
async fn import(app: &Application, path: &Path) -> String {
    app.import_trace(
        crate::TraceImportRequest {
            path: path.into(),
            password: None,
            max_file_bytes: 4 * 1024 * 1024,
            operation_id: transmog_capture::support::new_id().unwrap(),
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap()
    .trace
    .id
}
#[tokio::test]
async fn intake_uses_original_source_and_filters_before_virtual_paging() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("customer.tmcap");
    let marker = fixture(&path);
    let app = application(root.path());
    let source = import(&app, &path).await;
    let second = import(&app, &path).await;
    let intake = app.incident_intake(&source).unwrap();
    assert_eq!(intake.markers[0], marker);
    assert_eq!(intake.profile.unwrap().title, "Checkout failure");
    assert_eq!(intake.observed_user_agents, ["Observed agent fixture"]);
    assert!(
        intake
            .entered_client_version
            .starts_with("Customer-entered")
    );
    let window = IncidentWindow {
        trace_id: source.clone(),
        marker_id: marker.id,
        before_seconds: 10,
        after_seconds: 10,
        relevant_hosts_only: true,
    };
    let mut query = crate::SessionQueryInput {
        incident_window: Some(window),
        limit: Some(1),
        sort: Some(crate::SessionSort {
            column: crate::TrafficColumn::StartedAt,
            direction: crate::SortDirection::Ascending,
        }),
        ..Default::default()
    };
    let ids = app.matching_traffic_ids(&query).unwrap();
    assert_eq!(ids.len(), 2);
    let view = app.query_traffic_view(&query).unwrap();
    assert_eq!(view.ids.len(), 2);
    assert!(
        view.page
            .sessions
            .iter()
            .all(|s| s.trace_id.as_deref() == Some(&source))
    );
    assert_ne!(source, second);
    query.incident_window.as_mut().unwrap().relevant_hosts_only = false;
    assert_eq!(app.matching_traffic_ids(&query).unwrap().len(), 3);
    query.incident_window.as_mut().unwrap().marker_id = String::new();
    assert_eq!(app.matching_traffic_ids(&query).unwrap().len(), 4);
}
#[tokio::test]
#[allow(clippy::too_many_lines)] // End-to-end preservation across all export formats.
async fn export_review_freezes_membership_preserves_markers_and_omits_context_on_request() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("customer.tmcap");
    let marker = fixture(&path);
    let original_bytes = std::fs::read(&path).unwrap();
    let app = application(root.path());
    let source = import(&app, &path).await;
    let review = app
        .review_traffic_export(Some(crate::comparison::ComparisonScope::Source {
            trace_id: Some(source.clone()),
        }))
        .unwrap();
    assert_eq!(review.entries, 4);
    assert_eq!(review.incident_markers, 1);
    assert!(review.stream_records > 0);
    import(&app, &path).await;
    assert!(
        app.save_traffic_trace(path.clone(), crate::TraceSaveOptions::default())
            .await
            .is_err()
    );
    assert_eq!(original_bytes, std::fs::read(&path).unwrap());
    let output = root.path().join("reviewed.tmcap");
    let saved = app
        .save_traffic_trace(
            output.clone(),
            crate::TraceSaveOptions {
                review_id: Some(review.id.clone()),
                redact_sensitive_headers: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.entries, 4);
    assert_eq!(saved.omitted_incident_markers, 0);
    let reopened = application(root.path().join("reopened").as_path());
    import(&reopened, &output).await;
    let sources = reopened.trace_metadata_list();
    let original = sources
        .iter()
        .find(|s| s.context.get("incidentMarkers").is_some())
        .unwrap();
    assert_eq!(
        reopened.incident_intake(&original.id).unwrap().markers[0],
        marker
    );
    let no_context = root.path().join("without-context.tmcap");
    let saved = app
        .save_traffic_trace(
            no_context.clone(),
            crate::TraceSaveOptions {
                review_id: Some(review.id.clone()),
                include_original_context: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.omitted_incident_markers, 1);
    let stripped = application(root.path().join("stripped").as_path());
    import(&stripped, &no_context).await;
    assert!(
        stripped
            .trace_metadata_list()
            .iter()
            .all(|s| s.context.get("incidentMarkers").is_none())
    );
    for (format, extension, preserved) in [
        (crate::TraceSaveFormat::SazExtended, "extended.saz", true),
        (crate::TraceSaveFormat::SazStrict, "strict.saz", false),
        (crate::TraceSaveFormat::Har, "copy.har", false),
    ] {
        let copy = root.path().join(extension);
        let saved = app
            .save_traffic_trace(
                copy.clone(),
                crate::TraceSaveOptions {
                    format,
                    review_id: Some(review.id.clone()),
                    redact_sensitive_headers: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(saved.entries, 4);
        assert_eq!(saved.omitted_incident_markers, usize::from(!preserved));
        let copy_app = application(&root.path().join(extension.replace('.', "-")));
        import(&copy_app, &copy).await;
        let count = copy_app
            .trace_metadata_list()
            .iter()
            .map(|s| {
                s.context
                    .get("incidentMarkers")
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len)
            })
            .sum::<usize>();
        assert_eq!(count, usize::from(preserved));
    }
    assert_eq!(original_bytes, std::fs::read(&path).unwrap());
}
#[tokio::test]
async fn removed_review_entries_fail_before_overwriting_and_invalid_context_is_atomic() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("customer.tmcap");
    fixture(&path);
    let app = application(root.path());
    import(&app, &path).await;
    let id = app
        .query_sessions(crate::SessionQueryInput::default())
        .unwrap()
        .sessions[0]
        .id
        .clone();
    let review = app
        .review_traffic_export(Some(crate::comparison::ComparisonScope::Selected {
            ids: vec![id.clone()],
        }))
        .unwrap();
    app.remove_traffic_entries(&[id], false).unwrap();
    let output = root.path().join("keep.tmcap");
    std::fs::write(&output, b"existing user file").unwrap();
    assert!(
        app.save_traffic_trace(
            output.clone(),
            crate::TraceSaveOptions {
                review_id: Some(review.id.clone()),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(output).unwrap(), b"existing user file");
    let invalid = root.path().join("invalid.tmcap");
    let mut writer =
        CaptureWriter::new(File::create(&invalid).unwrap(), CaptureLimits::default()).unwrap();
    writer
        .append(&Record {
            sequence: 0,
            exchange_id: 0,
            kind: Kind::Unknown {
                kind: "trace-metadata".into(),
                payload: serde_json::json!({"incidentMarkers":[{"version":2,"note":"malformed"}]}),
            },
        })
        .unwrap();
    writer.seal().unwrap();
    let before = app.trace_metadata_list().len();
    assert!(
        app.import_trace(
            crate::TraceImportRequest {
                path: invalid,
                password: None,
                max_file_bytes: 1024 * 1024,
                operation_id: "invalid".into()
            },
            Arc::new(|_| {})
        )
        .await
        .is_err()
    );
    assert_eq!(before, app.trace_metadata_list().len());
}
