use super::*;
use crate::{AppConfig, BodyStoreConfig, ErrorCategory};
use std::{fs::File, path::PathBuf};
use transmog_capture::{CaptureLimits, CaptureReader, CaptureRecordKind};
use transmog_core::{intercept::ExchangeId, observe::ExchangeBoundary};
fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../verification/fixtures/portable")
}
fn application(root: &Path) -> Application {
    Application::new(AppConfig {
        body_store: Some(BodyStoreConfig::product_default(root.join("bodies"))),
        response_asset_root: Some(root.join("assets")),
        automation_path: Some(root.join("automation")),
        ..Default::default()
    })
    .unwrap()
}
fn manifest(name: &str) -> wire::CaseManifest {
    wire::CaseManifest::parse(
        &std::fs::read(corpus().join("manifests").join(format!("{name}.json"))).unwrap(),
    )
    .unwrap()
}
fn prepare(value: &wire::CaseManifest, stage: &Path) -> wire::PreparedCase {
    wire::PreparedCase::prepare(
        &value.encode().unwrap(),
        stage,
        |sha| {
            Ok(Box::new(File::open(
                corpus().join("objects/sha256").join(sha),
            )?))
        },
        &AtomicBool::new(false),
    )
    .unwrap()
}
fn request() -> CaseExportRequest {
    CaseExportRequest {
        rules: RuleExportRequest {
            id: "case-export".into(),
            revision: "1".into(),
            title: "Working investigation".into(),
            rule_ids: vec![],
            selections: vec![],
        },
        evidence_review_id: None,
        derived_plaintext: false,
        notes: vec![],
    }
}

#[tokio::test]
async fn encrypted_package_inspects_without_extraction_and_keeps_pinned_bodies_after_close() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let value = manifest("source-collision");
    let prepared = prepare(&value, stage.path());
    let path = root.path().join("case.tmcase.tmcap");
    let password = transmog_capture::CapturePassword::new("package-fixture".into());
    wire::write_case_package(
        &prepared,
        &path,
        wire::PackageFormat::Native,
        Some(password.clone()),
        false,
        &AtomicBool::new(false),
    )
    .unwrap();
    drop(prepared);
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
    let package = wire::Package::open(&path, Some(&password), &AtomicBool::new(false)).unwrap();
    let before = app.automation_status();
    let opened = app
        .open_package_evidence(&package, "source-a", None, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert_eq!(app.automation_status(), before);
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
    let id = opened
        .resolve(value.notes[0].source.as_ref().unwrap())
        .unwrap()
        .to_owned();
    drop(package);
    std::fs::remove_file(&path).unwrap();
    let bytes = app
        .body_store
        .as_ref()
        .unwrap()
        .read_range(
            ExchangeId(u128::from_str_radix(&id, 16).unwrap()),
            ExchangeBoundary::ClientResponse,
            0,
            1,
        )
        .unwrap();
    assert_eq!(bytes.bytes, b"h");
    let mut request = request();
    request.derived_plaintext = true;
    request.evidence_review_id = Some(app.review_traffic_export(None).unwrap().id);
    let derived = app
        .prepare_investigation(&request, stage.path(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        derived.manifest().sources[0].lineage.parents[0].artifact_sha256,
        value.sources[0].artifact_sha256
    );
}

#[tokio::test]
async fn mismatched_native_source_identity_fails_before_traffic_publication() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let value = manifest("source-collision");
    let bytes = String::from_utf8(value.encode().unwrap())
        .unwrap()
        .replace("\"sourceKey\": \"root\"", "\"sourceKey\": \"absent\"");
    let value = wire::CaseManifest::parse(bytes.as_bytes()).unwrap();
    let prepared = prepare(&value, stage.path());
    assert!(
        app.open_investigation_evidence(
            &prepared,
            "source-a",
            None,
            Arc::new(AtomicBool::new(false))
        )
        .await
        .is_err()
    );
    assert!(app.trace_metadata_list().is_empty());
    assert_eq!(app.automation_status().generation, 0);
}

#[tokio::test]
async fn unavailable_optional_source_context_does_not_block_present_evidence() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let mut value = manifest("network-context");
    let source_id = value.sources[0].id.clone();
    let mut omitted = value.sources[0].clone();
    omitted.id = "omitted-source".into();
    omitted.source_key = "not-retained".into();
    omitted.dependency = wire::Availability {
        state: "missing".into(),
        reason: Some("The original source was not retained".into()),
        sha256: None,
        bytes: None,
    };
    let mut context = value.contexts[0].clone();
    context.id = "omitted-context".into();
    context.kind = "capture-metadata".into();
    context.origin.kind = "captured-source".into();
    context.origin.source = Some(wire::SourceAssociation {
        artifact_sha256: omitted.artifact_sha256.clone(),
        source_key: omitted.source_key.clone(),
    });
    value.sources.push(omitted);
    value.contexts.push(context);
    let prepared = prepare(&value, stage.path());
    let opened = app
        .open_investigation_evidence(
            &prepared,
            &source_id,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    assert!(!opened.links.is_empty());
    assert_eq!(app.automation_status().generation, 0);
}

#[tokio::test]
async fn colliding_sources_open_inertly_with_distinct_links_and_pinned_body_lifetimes() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let value = manifest("source-collision");
    let case = prepare(&value, stage.path());
    let first = app
        .open_investigation_evidence(&case, "source-a", None, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    let second = app
        .open_investigation_evidence(&case, "source-b", None, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    let original = &value.notes[0].source.as_ref().unwrap();
    let id = first.resolve(original).unwrap().to_owned();
    assert!(second.resolve(original).is_none());
    assert_ne!(first.links[0].exchange_id, second.links[0].exchange_id);
    assert_eq!(
        app.traces
            .artifact_origin(&first.imported.trace.id, &AtomicBool::new(false))
            .unwrap()
            .unwrap()
            .0,
        value.sources[0].artifact_sha256
    );
    assert_eq!(app.automation_status().generation, 0);
    assert!(app.automation_status().rules.is_empty());
    drop(case);
    let bytes = app
        .body_store
        .as_ref()
        .unwrap()
        .read_range(
            ExchangeId(u128::from_str_radix(&id, 16).unwrap()),
            ExchangeBoundary::ClientResponse,
            0,
            1,
        )
        .unwrap();
    assert_eq!(bytes.bytes, b"h");
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Exercise the complete source-context/native-stream round trip together.
async fn selected_native_export_moves_context_to_objects_and_preserves_streams_loss_and_notes() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let mut value = manifest("simple-css");
    let source = wire::SourceAssociation {
        artifact_sha256: value.sources[0].artifact_sha256.clone(),
        source_key: "root".into(),
    };
    let context = serde_json::json!({"context":{"networkContext":{"collector":"Synthetic","purpose":"capture","platform":"windows","collectedAt":1_800_000_000_000_u64,"command":"inert fixture","output":"Café\r\nroute output","notes":[]},"incidentMarkers":[{"version":1,"id":"11111111111111111111111111111111","runId":"22222222222222222222222222222222","unixMillis":1_800_000_000_000_u64,"offsetMillis":null,"origin":"cli","creationKind":"user-mark","note":"Synthetic incident"}]},"notes":["Original machine context"],"format":"native"});
    let bytes = serde_json::to_vec(&context).unwrap();
    let sha = wire::sha256(&bytes);
    value.objects.push(wire::Object {
        sha256: sha.clone(),
        bytes: bytes.len().to_string(),
        role: "context".into(),
        media_type: "application/json".into(),
    });
    value.contexts.push(wire::CaseContext {
        id: "original-context".into(),
        kind: "capture-metadata".into(),
        origin: wire::ContextOrigin {
            kind: "captured-source".into(),
            collected_at: None,
            collector: wire::Producer {
                name: "unknown".into(),
                version: "unknown".into(),
            },
            platform: "unknown".into(),
            purpose: "preserved-source-context".into(),
            machine_label: None,
            source: Some(source),
        },
        content: wire::Availability {
            state: "present".into(),
            reason: None,
            sha256: Some(sha.clone()),
            bytes: Some(bytes.len().to_string()),
        },
        completeness: "unknown".into(),
        issues: vec![],
    });
    let case = wire::PreparedCase::prepare(
        &value.encode().unwrap(),
        stage.path(),
        |digest| {
            if digest == sha {
                Ok(Box::new(std::io::Cursor::new(bytes.clone())) as Box<dyn Read + Send>)
            } else {
                Ok(Box::new(File::open(
                    corpus().join("objects/sha256").join(digest),
                )?))
            }
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let opened = app
        .open_investigation_evidence(&case, "source-a", None, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    let review = app.review_traffic_export(None).unwrap();
    let mut options = request();
    options.evidence_review_id = Some(review.id);
    options.derived_plaintext = true;
    options.notes = vec![CaseNoteInput {
        id: "reproduction".into(),
        text: "Authored steps <script>inert</script>".into(),
        exchange_id: Some(opened.links[0].exchange_id.clone()),
    }];
    let prepared = app
        .prepare_investigation(&options, stage.path(), &AtomicBool::new(false))
        .unwrap();
    let saved = prepared.manifest();
    assert_eq!(
        saved.sources[0].lineage.parents[0].artifact_sha256,
        value.sources[0].artifact_sha256
    );
    assert_eq!(
        saved.sources[0].lineage.parents[0].source_key.as_deref(),
        Some("root")
    );
    assert_eq!(saved.sources.len(), 1);
    assert_eq!(saved.contexts.len(), 1);
    assert_eq!(saved.markers.len(), 1);
    assert!(saved.markers[0].elapsed_millis.is_none());
    assert!(saved.contexts[0].origin.collected_at.is_none());
    assert_eq!(
        saved.notes[0].source.as_ref().unwrap().artifact_sha256,
        saved.sources[0].artifact_sha256
    );
    assert!(
        !saved.sources[0]
            .source_key
            .contains(&opened.imported.trace.id)
    );
    let object = saved.objects.iter().find(|o| o.role == "capture").unwrap();
    let mut reader = CaptureReader::new(
        prepared.open_object(&object.sha256).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    let mut stream = false;
    let mut loss = false;
    while let Some(frame) = reader.read_next().unwrap() {
        match frame.record.kind {
            CaptureRecordKind::StreamMessage(_) => stream = true,
            CaptureRecordKind::Loss { .. } => loss = true,
            CaptureRecordKind::Unknown { kind, payload } if kind == "trace-source" => {
                assert!(payload["context"].is_null());
                assert_eq!(payload["path"], "");
            }
            _ => {}
        }
    }
    assert!(stream && loss);
    let viewer_root = tempfile::tempdir().unwrap();
    let viewer = application(viewer_root.path());
    let reopened = viewer
        .open_investigation_evidence(
            &prepared,
            &saved.sources[0].id,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    assert!(
        reopened
            .resolve(saved.notes[0].source.as_ref().unwrap())
            .is_some()
    );
    let metadata = viewer
        .trace_metadata_list()
        .into_iter()
        .find(|s| s.context["networkContext"].is_object())
        .unwrap();
    assert_eq!(
        metadata.context["networkContext"]["output"],
        "Café\r\nroute output"
    );
    assert_eq!(
        viewer.incident_intake(&metadata.id).unwrap().markers.len(),
        1
    );
    assert_eq!(viewer.automation_status().generation, 0);
    let mut without_context = saved.clone();
    without_context.contexts.clear();
    without_context
        .objects
        .retain(|object| object.role != "context");
    let bare = wire::PreparedCase::prepare(
        &without_context.encode().unwrap(),
        stage.path(),
        |sha| Ok(Box::new(prepared.open_object(sha)?)),
        &AtomicBool::new(false),
    )
    .unwrap();
    let another_root = tempfile::tempdir().unwrap();
    let another = application(another_root.path());
    another
        .open_investigation_evidence(
            &bare,
            &bare.manifest().sources[0].id,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    let mut selected = request();
    selected.evidence_review_id = Some(another.review_traffic_export(None).unwrap().id);
    selected.derived_plaintext = true;
    let again = another
        .prepare_investigation(&selected, stage.path(), &AtomicBool::new(false))
        .unwrap();
    let object = again
        .manifest()
        .objects
        .iter()
        .find(|o| o.role == "capture")
        .unwrap();
    let mut reader = CaptureReader::new(
        again.open_object(&object.sha256).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    let mut missing = 0;
    while let Some(frame) = reader.read_next().unwrap() {
        if let CaptureRecordKind::Loss { count, .. } = frame.record.kind {
            missing += count;
        }
    }
    assert_eq!(missing, 2); // Actual capture-wide loss survives even without descriptive sidecars.
}

#[tokio::test]
async fn locked_or_canceled_case_opening_never_adds_partial_traffic() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let case = prepare(&manifest("encrypted-capture"), stage.path());
    let error = app
        .open_investigation_evidence(&case, "source-a", None, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    assert_eq!(error.category, ErrorCategory::PasswordRequired);
    assert!(app.service.catalog().project_retained(|_| ()).is_empty());
    let error = app
        .open_investigation_evidence(
            &case,
            "source-a",
            Some(transmog_capture::CapturePassword::new(
                "portable-fixture-password".into(),
            )),
            Arc::new(AtomicBool::new(true)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category, ErrorCategory::Canceled);
    let opened = app
        .open_investigation_evidence(
            &case,
            "source-a",
            Some(transmog_capture::CapturePassword::new(
                "portable-fixture-password".into(),
            )),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    assert!(!opened.links.is_empty());
}
#[test]
fn explicit_plaintext_review_and_selection_are_required_without_implicit_traffic_or_automation() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let mut options = request();
    options.evidence_review_id = Some("unreviewed".into());
    assert!(
        app.prepare_investigation(&options, stage.path(), &AtomicBool::new(false))
            .is_err()
    );
    options.evidence_review_id = None;
    let case = app
        .prepare_investigation(&options, stage.path(), &AtomicBool::new(false))
        .unwrap();
    assert!(case.manifest().objects.is_empty());
    assert!(case.manifest().sources.is_empty());
    options.notes = vec![CaseNoteInput {
        id: "outside".into(),
        text: "Missing selection".into(),
        exchange_id: Some("unselected".into()),
    }];
    assert!(
        app.prepare_investigation(&options, stage.path(), &AtomicBool::new(false))
            .is_err()
    );
    assert!(app.automation_status().rules.is_empty());
    assert_eq!(app.automation_status().generation, 0);
}

#[tokio::test]
async fn changed_original_input_is_rejected_without_changing_the_workspace() {
    use std::io::Write as _;
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let path = root.path().join("original.tmcap");
    std::fs::copy(corpus().join("seeds/source-a.tmcap"), &path).unwrap();
    app.import_trace(
        crate::TraceImportRequest {
            path: path.clone(),
            password: None,
            operation_id: "original".into(),
            max_file_bytes: u64::MAX,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    let review = app.review_traffic_export(None).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"changed after import")
        .unwrap();
    let mut options = request();
    options.evidence_review_id = Some(review.id);
    options.derived_plaintext = true;
    assert!(
        app.prepare_investigation(&options, stage.path(), &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(app.automation_status().generation, 0);
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn repeated_original_entry_occurrences_survive_clear_and_selected_derivation() {
    use transmog_capture::{CaptureRecord, CaptureWriter, CapturedHeader};
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let mut bytes = Vec::new();
    let mut writer = CaptureWriter::new(&mut bytes, CaptureLimits::default()).unwrap();
    writer.append(&CaptureRecord{exchange_id:0,sequence:1,kind:CaptureRecordKind::Unknown{kind:"trace-source".into(),payload:serde_json::json!({"id":"same-source","name":"Original","format":"native","path":"","sessions":2,"importedAt":0,"context":null,"notes":[]})}}).unwrap();
    for id in [8, 3] {
        writer
            .append(&CaptureRecord {
                exchange_id: id,
                sequence: 1,
                kind: CaptureRecordKind::RequestHead {
                    boundary: ExchangeBoundary::ClientRequest,
                    method: "GET".into(),
                    target: format!("https://portable.invalid/{id}"),
                    headers: Vec::<CapturedHeader>::new(),
                },
            })
            .unwrap();
        writer.append(&CaptureRecord{exchange_id:id,sequence:2,kind:CaptureRecordKind::Unknown{kind:"entry-provenance".into(),payload:serde_json::json!({"source":{"traceId":"same-source","originalId":"same-original","rawHeaders":null,"timings":{},"protocolKnown":true,"targetKnown":true,"diagnostics":[]},"terminalUnixMillis":null})}}).unwrap();
    }
    writer.seal().unwrap();
    let digest = wire::sha256(&bytes);
    let mut value = manifest("simple-css");
    value.objects = vec![wire::Object {
        sha256: digest.clone(),
        bytes: bytes.len().to_string(),
        role: "capture".into(),
        media_type: "application/vnd.transmog.tmcap".into(),
    }];
    value.sources[0].artifact_sha256 = digest.clone();
    value.sources[0].source_key = "same-source".into();
    value.assets.clear();
    value.experiments.clear();
    value.notes.clear();
    let prepared = wire::PreparedCase::prepare(
        &value.encode().unwrap(),
        stage.path(),
        |_| Ok(Box::new(std::io::Cursor::new(bytes.clone()))),
        &AtomicBool::new(false),
    )
    .unwrap();
    let opened = app
        .open_investigation_evidence(
            &prepared,
            "source-a",
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    assert_eq!(
        opened
            .links
            .iter()
            .map(|l| l.reference.occurrence.as_str())
            .collect::<Vec<_>>(),
        ["0", "1"]
    );
    let first = ExchangeId(u128::from_str_radix(&opened.links[0].exchange_id, 16).unwrap());
    app.service.catalog().dismiss(&[first]);
    app.traces.forget_entries(&[first]);
    let review = app
        .review_traffic_export(Some(crate::comparison::ComparisonScope::Selected {
            ids: vec![opened.links[1].exchange_id.clone()],
        }))
        .unwrap();
    let mut options = request();
    options.evidence_review_id = Some(review.id);
    options.derived_plaintext = true;
    let exported = app
        .prepare_investigation(&options, stage.path(), &AtomicBool::new(false))
        .unwrap();
    let mapping = &exported.manifest().sources[0].lineage.entry_mappings[0];
    assert_eq!(mapping.occurrence, "0");
    assert_eq!(mapping.original.occurrence, "1");
    assert_eq!(mapping.original.artifact_sha256, digest);
    assert_eq!(mapping.original.source_key, "same-source");
}
