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
async fn marker_edits_are_isolated_revisioned_and_roundtrip_without_old_text() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("original.tmcap");
    let marker = fixture(&path);
    let original = std::fs::read(&path).unwrap();
    let app = application(root.path());
    let source = import(&app, &path).await;
    let duplicate = import(&app, &path).await;
    let review = app
        .review_traffic_export(Some(crate::comparison::ComparisonScope::Source {
            trace_id: Some(source.clone()),
        }))
        .unwrap();
    let edited = app
        .change_trace_marker(
            &source,
            0,
            MarkerChange::Edit {
                id: marker.id.clone(),
                note: "Reviewer: checkout failed after submit.".into(),
            },
        )
        .unwrap();
    assert!(edited.dirty);
    assert_eq!(edited.markers[0].unix_millis, marker.unix_millis);
    assert_eq!(edited.markers[0].origin, marker.origin);
    assert_eq!(app.incident_intake(&duplicate).unwrap().markers[0], marker);
    assert!(
        app.change_trace_marker(
            &source,
            0,
            MarkerChange::Delete {
                id: marker.id.clone()
            }
        )
        .is_err()
    );
    assert!(app.export_scope(Some(&review.id)).is_err());
    let output = root.path().join("reviewed.tmcap");
    let current = app
        .review_traffic_export(Some(crate::comparison::ComparisonScope::Source {
            trace_id: Some(source.clone()),
        }))
        .unwrap();
    app.save_traffic_trace(
        output.clone(),
        crate::TraceSaveOptions {
            review_id: Some(current.id),
            include_original_context: false,
            include_markers: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!app.traces.marker_review(&source).unwrap().dirty);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let reopened = application(&root.path().join("reopened"));
    import(&reopened, &output).await;
    let sources = reopened.trace_metadata_list();
    let source = sources
        .iter()
        .find(|s| s.context.get("incidentMarkers").is_some())
        .unwrap();
    let context = serde_json::to_string(&source.context).unwrap();
    assert!(!context.contains(&marker.note));
    assert!(source.context.get("supportCapture").is_none());
    let intake = reopened.incident_intake(&source.id).unwrap();
    assert_eq!(
        intake.markers[0].note,
        "Reviewer: checkout failed after submit."
    );
    assert_eq!(intake.review.edited_marker_ids, [marker.id]);
}

#[tokio::test]
async fn clear_delete_undo_and_marker_only_exports_keep_the_review_available() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("original.tmcap");
    let marker = fixture(&path);
    let app = application(root.path());
    let source = import(&app, &path).await;
    let cleared = app
        .change_trace_marker(
            &source,
            0,
            MarkerChange::Edit {
                id: marker.id.clone(),
                note: String::new(),
            },
        )
        .unwrap();
    assert_eq!(cleared.markers.len(), 1);
    assert!(cleared.markers[0].note.is_empty());
    let deleted = app
        .change_trace_marker(
            &source,
            cleared.revision,
            MarkerChange::Delete {
                id: marker.id.clone(),
            },
        )
        .unwrap();
    assert!(deleted.markers.is_empty());
    assert_eq!(deleted.deleted_markers, 1);
    let restored = app
        .change_trace_marker(&source, deleted.revision, MarkerChange::Undo)
        .unwrap();
    assert_eq!(restored.markers.len(), 1);
    assert!(restored.markers[0].note.is_empty());
    let original = app
        .change_trace_marker(&source, restored.revision, MarkerChange::Undo)
        .unwrap();
    assert_eq!(original.markers[0], marker);
    assert!(!original.dirty);
    app.change_trace_marker(
        &source,
        original.revision,
        MarkerChange::Delete {
            id: marker.id.clone(),
        },
    )
    .unwrap();
    let ids = app
        .matching_traffic_ids(&crate::SessionQueryInput::default())
        .unwrap();
    app.remove_traffic_entries(&ids, false).unwrap();
    let review = app
        .review_traffic_export(Some(crate::comparison::ComparisonScope::Source {
            trace_id: Some(source.clone()),
        }))
        .unwrap();
    assert_eq!(review.entries, 0);
    assert_eq!(review.deleted_markers, 1);
    let output = root.path().join("empty.tmcap");
    app.save_traffic_trace(
        output.clone(),
        crate::TraceSaveOptions {
            review_id: Some(review.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let reopened = application(&root.path().join("reopened"));
    import(&reopened, &output).await;
    assert!(
        reopened
            .trace_marker_reviews()
            .unwrap()
            .iter()
            .all(|r| r.markers.is_empty())
    );
    assert!(!app.traces.marker_review(&source).unwrap().dirty);
}

#[tokio::test]
async fn incompatible_export_and_late_edits_do_not_clear_unsaved_state() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("original.tmcap");
    let marker = fixture(&path);
    let app = application(root.path());
    let source = import(&app, &path).await;
    app.change_trace_marker(
        &source,
        0,
        MarkerChange::Edit {
            id: marker.id.clone(),
            note: "First revision".into(),
        },
    )
    .unwrap();
    let snapshot = app.trace_metadata_list();
    app.change_trace_marker(
        &source,
        1,
        MarkerChange::Edit {
            id: marker.id.clone(),
            note: "Newer revision".into(),
        },
    )
    .unwrap();
    app.traces.markers_saved(&snapshot).unwrap();
    assert!(app.has_marker_changes());
    let result = app
        .save_traffic_trace(
            root.path().join("copy.har"),
            crate::TraceSaveOptions {
                format: crate::TraceSaveFormat::Har,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.omitted_incident_markers, 1);
    assert!(app.has_marker_changes());
    app.discard_marker_changes().unwrap();
    assert!(!app.has_marker_changes());
    assert_eq!(
        app.incident_intake(&source).unwrap().markers[0].note,
        "First revision"
    );
}

#[tokio::test]
async fn flagged_messages_reopen_with_exact_links_in_supported_copies() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("original.tmcap");
    fixture(&path);
    let app = application(root.path());
    import(&app, &path).await;
    let ids = app
        .matching_traffic_ids(&crate::SessionQueryInput::default())
        .unwrap();
    let flagged = app
        .add_trace_marker(&ids[0], "response", "Reviewer response note".into())
        .unwrap();
    let marker = flagged.markers.last().unwrap();
    assert_eq!(
        marker.origin,
        transmog_capture::support::MarkerOrigin::Analyst
    );
    assert_eq!(
        marker.creation_kind,
        transmog_capture::support::CreationKind::LaterAnnotation
    );
    assert_eq!(flagged.marker_entries.get(&marker.id), Some(&ids[0]));
    assert_eq!(flagged.links[0].boundary, "response");
    for (format, extension) in [
        (crate::TraceSaveFormat::Native, "tmcap"),
        (crate::TraceSaveFormat::SazExtended, "saz"),
    ] {
        let review = app
            .review_traffic_export(Some(crate::comparison::ComparisonScope::Selected {
                ids: vec![ids[0].clone()],
            }))
            .unwrap();
        let output = root.path().join(format!("flagged.{extension}"));
        app.save_traffic_trace(
            output.clone(),
            crate::TraceSaveOptions {
                review_id: Some(review.id),
                format,
                include_original_context: false,
                include_markers: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let reopened = application(&root.path().join(extension));
        import(&reopened, &output).await;
        let review = reopened
            .trace_marker_reviews()
            .unwrap()
            .into_iter()
            .find(|r| r.links.iter().any(|link| link.marker_id == marker.id))
            .unwrap();
        let id = review
            .marker_entries
            .get(&marker.id)
            .expect("reopened exact request link");
        assert!(
            reopened
                .matching_traffic_ids(&crate::SessionQueryInput::default())
                .unwrap()
                .contains(id)
        );
        assert_eq!(
            review
                .markers
                .iter()
                .find(|m| m.id == marker.id)
                .unwrap()
                .note,
            "Reviewer response note"
        );
    }
}

#[tokio::test]
async fn flagging_live_requests_keeps_live_scopes_and_exports_their_markers() {
    use transmog_core::{
        ClientIdentity, ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{ExchangeId, ExchangeMetadata},
        observe::{ObserverEvent, ObserverEventKind},
    };
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let session = SessionMetadata {
        session_id: SessionId(1),
        downstream_connection_id: ConnectionId(1),
        stream_id: StreamId(1),
        client_addr: "127.0.0.1:1".parse().unwrap(),
        client_identity: ClientIdentity::default(),
        proxy_addr: "127.0.0.1:2".parse().unwrap(),
        ingress_version: HttpLegVersion::Http1,
        egress_version: None,
    };
    let target = Target {
        scheme: "https".into(),
        authority: "example.test".into(),
        host: "example.test".into(),
        port: 443,
        path: "/".into(),
        query: None,
    };
    app.service.catalog().apply(ObserverEvent {
        exchange_id: ExchangeId(1),
        sequence: 1,
        kind: ObserverEventKind::ExchangeStarted {
            metadata: Arc::new(ExchangeMetadata::from_session(&session, target)),
        },
    });
    let id = format!("{:032x}", 1);
    assert!(
        app.add_trace_marker(&id, "response", String::new())
            .is_err()
    );
    let review = app.add_trace_marker(&id, "request", String::new()).unwrap();
    assert_eq!(review.trace_id, "live");
    assert_eq!(review.added_markers, 1);
    assert!(app.traces.source_id(&id).is_none());
    for source in [None, Some("live".into())] {
        let scope = crate::comparison::ComparisonScope::Source { trace_id: source };
        let found = crate::export_review::scope_ids(&app, &scope).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], id);
    }
    let export = app
        .review_traffic_export(Some(crate::comparison::ComparisonScope::Selected {
            ids: vec![id.clone()],
        }))
        .unwrap();
    assert_eq!(export.incident_markers, 1);
    let output = root.path().join("live.tmcap");
    app.save_traffic_trace(
        output.clone(),
        crate::TraceSaveOptions {
            review_id: Some(export.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let reopened = application(&root.path().join("reopened"));
    import(&reopened, &output).await;
    let reviews = reopened.trace_marker_reviews().unwrap();
    let source = reviews.iter().find(|r| !r.markers.is_empty()).unwrap();
    assert_eq!(source.marker_entries.len(), 1);
    assert!(source.markers[0].note.is_empty());
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
    let counts = app
        .traffic_facets(&crate::TrafficFacetInput {
            include_values: vec![],
            query: query.clone(),
            dimension: crate::TrafficDimension::Column(crate::TrafficColumn::Method),
            search: None,
            offset: 0,
            limit: None,
        })
        .unwrap();
    assert_eq!(counts.matching_entries, 2);
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
