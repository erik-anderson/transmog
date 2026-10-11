use super::*;
use std::path::Path;
use transmog_capture::{CaptureRecord, CaptureWriter};

fn write_records(path: &Path, records: Vec<CaptureRecord>) {
    let mut writer =
        CaptureWriter::new(File::create(path).unwrap(), CaptureLimits::default()).unwrap();
    for record in records {
        writer.append(&record).unwrap();
    }
    writer.seal().unwrap();
}

fn record(kind: CaptureRecordKind) -> CaptureRecord {
    CaptureRecord {
        exchange_id: 1,
        sequence: 1,
        kind,
    }
}

fn read(path: &Path) -> Result<ImportData, AppError> {
    TraceRegistry::read(&request(path), &AtomicBool::new(false), &|_| {}).map(|(data, _, _)| data)
}

fn request(path: &Path) -> TraceImportRequest {
    TraceImportRequest {
        path: path.into(),
        password: None,
        operation_id: "native-review".into(),
        max_file_bytes: u64::MAX,
    }
}

fn body(count: usize, bytes: Option<Vec<u8>>) -> CaptureRecord {
    record(CaptureRecordKind::BodySegment {
        boundary: ExchangeBoundary::ClientRequest,
        byte_count: count,
        bytes,
        truncated: false,
    })
}

fn head(target: String) -> CaptureRecord {
    record(CaptureRecordKind::RequestHead {
        boundary: ExchangeBoundary::ClientRequest,
        method: "POST".into(),
        target,
        headers: vec![],
    })
}

#[test]
fn native_rejects_retained_bytes_exceeding_segment_observation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("counts.tmcap");
    for count in [0, 1] {
        write_records(
            &path,
            vec![
                head("https://example.invalid/".into()),
                body(count, Some(vec![1, 2])),
                record(CaptureRecordKind::Completed),
            ],
        );
        assert!(
            read(&path).is_err(),
            "accepted two saved bytes with byte_count={count}"
        );
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn native_rejects_overflowing_observed_body_count() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("overflow.tmcap");
    write_records(&path, vec![body(usize::MAX, None), body(1, None)]);
    assert!(read(&path).is_err());
}

#[test]
fn native_charges_retained_target_and_caller_strings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("strings.tmcap");
    for caller in [false, true] {
        let mut charges = Vec::new();
        for size in [0, 16 * 1024] {
            let record = if caller {
                record(CaptureRecordKind::ExchangeStarted {
                    client_addr: "127.0.0.1:1234".into(),
                    client_identity: ClientIdentity::LocalProcess {
                        pid: 7,
                        name: Some("x".repeat(size)),
                    },
                    listener_addr: "127.0.0.1:8888".into(),
                    authority: "example.invalid".into(),
                    started_unix_nanos: 0,
                })
            } else {
                head(format!("https://example.invalid/{}", "x".repeat(size)))
            };
            write_records(&path, vec![record]);
            charges.push(read(&path).unwrap().index_bytes);
        }
        assert!(
            charges[1] >= charges[0] + 16 * 1024,
            "uncharged caller={caller}: {charges:?}"
        );
    }
}

#[test]
fn native_charges_shared_certificate_der_once_per_import() {
    let ca = transmog_tls::ProxyCa::generate("Native import accounting test", 1).unwrap();
    let der = ca.certificate().to_der().unwrap();
    let evidence = transmog_core::performance::PerformanceEvidence {
        transports: vec![transmog_core::performance::TransportObservation {
            leg: "upstream".into(),
            connection_id: "shared".into(),
            tls: Some(transmog_core::TlsEvidence {
                server_chain: vec![transmog_core::PublicCertificate {
                    sha256: format!("{:x}", Sha256::digest(&der)),
                    der: der.into(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    };
    let certificate_bytes: usize = evidence
        .transports
        .iter()
        .filter_map(|transport| transport.tls.as_ref())
        .flat_map(|tls| tls.server_chain.iter().chain(&tls.client_chain))
        .map(|certificate| certificate.der.len())
        .sum();
    assert!(certificate_bytes > 0);
    let serialized_bytes = serde_json::to_vec(&evidence).unwrap().len();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("certificates.tmcap");
    // Subtract an otherwise identical row's fixed cost, leaving only performance evidence.
    write_records(&path, vec![record(CaptureRecordKind::Completed)]);
    let fixed = read(&path).unwrap().index_bytes;
    let mut one = record(CaptureRecordKind::Performance(evidence));
    write_records(&path, vec![one.clone()]);
    let first = read(&path).unwrap().index_bytes;
    one.exchange_id = 2;
    write_records(
        &path,
        vec![
            CaptureRecord {
                exchange_id: 1,
                ..one.clone()
            },
            one,
        ],
    );
    let twice = read(&path).unwrap().index_bytes;
    assert_eq!(first - fixed, serialized_bytes + certificate_bytes);
    assert_eq!(twice - 2 * fixed, 2 * serialized_bytes + certificate_bytes);
}

#[test]
fn native_decoded_representation_can_exceed_original_wire_size() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("decoded.tmcap");
    write_records(
        &path,
        vec![
            record(CaptureRecordKind::Unknown {
                kind: "body-representation".into(),
                payload: serde_json::json!({"boundary":"client-request", "contentCodings":[], "observedBytes":1, "retainedBytes":3, "complete":true}),
            }),
            body(3, Some(b"abc".to_vec())),
            record(CaptureRecordKind::Completed),
        ],
    );
    let imported = read(&path).unwrap();
    assert!(imported.bodies[0].complete);
    assert_eq!(imported.bodies[0].observed, 1);
    assert_eq!(imported.bodies[0].source.length(), Some(3));
}

#[test]
fn native_rejects_mismatched_head_boundaries_and_invalid_status() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("heads.tmcap");
    let mut wrong_request = head("https://example.invalid/".into());
    if let CaptureRecordKind::RequestHead { boundary, .. } = &mut wrong_request.kind {
        *boundary = ExchangeBoundary::ClientResponse;
    }
    let cases = std::iter::once(wrong_request).chain(
        [
            (ExchangeBoundary::ClientRequest, 200),
            (ExchangeBoundary::ClientResponse, 99),
            (ExchangeBoundary::ClientResponse, 1000),
        ]
        .into_iter()
        .map(|(boundary, status)| {
            record(CaptureRecordKind::ResponseHead {
                boundary,
                status,
                headers: vec![],
            })
        }),
    );
    for bad in cases {
        write_records(&path, vec![bad]);
        assert!(read(&path).is_err());
    }
}

#[test]
fn native_metadata_growth_enforces_budget_before_retention() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("budget.tmcap");
    write_records(
        &path,
        vec![head(format!(
            "https://example.invalid/{}",
            "x".repeat(2048)
        ))],
    );
    let mut capture =
        CaptureReader::new(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
    let frame = capture.read_next_indexed().unwrap().unwrap();
    let mut row = NativeSession::default();
    let mut budget = MAX_HEAD_BYTES - 1024;
    assert!(apply_native_frame(&mut row, frame, &mut budget, &mut HashSet::new()).is_err());
    assert!(row.requests.is_empty());
}

fn app(root: &Path) -> crate::Application {
    crate::Application::new(crate::AppConfig {
        body_store: Some(crate::BodyStoreConfig::product_default(root.join("cache"))),
        ..crate::AppConfig::default()
    })
    .unwrap()
}

fn assert_empty(app: &crate::Application, id: ExchangeId) {
    assert!(app.trace_metadata_list().is_empty());
    assert!(
        app.query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions
            .is_empty()
    );
    assert!(app.body_store().unwrap().metadata(id).is_empty());
    assert_eq!(app.traces.retained_index_bytes(), 0);
}

#[test]
fn cancellation_while_waiting_for_publication_has_no_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cancel.tmcap");
    write_records(
        &path,
        vec![
            body(3, Some(b"abc".to_vec())),
            record(CaptureRecordKind::Completed),
        ],
    );
    let workspace = app(root.path());
    let canceled = Arc::new(AtomicBool::new(false));
    let (data, trace, _) = TraceRegistry::read(&request(&path), &canceled, &|_| {}).unwrap();
    let id = data.sessions[0].exchange_id;
    let publication = workspace.traces.publication.lock().unwrap();
    let other = workspace.clone();
    let worker_canceled = canceled.clone();
    let (ready, started) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        ready.send(()).unwrap();
        other.traces.publish_import(
            data,
            &other.service,
            other.body_store().unwrap(),
            trace,
            &worker_canceled,
        )
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    canceled.store(true, Ordering::Release);
    drop(publication);
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.message.contains("canceled"));
    assert_empty(&workspace, id);
}

#[tokio::test]
async fn dropping_import_future_cancels_worker_and_releases_slot_when_it_exits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("aborted.tmcap");
    write_records(
        &path,
        vec![
            body(3, Some(b"abc".to_vec())),
            record(CaptureRecordKind::Completed),
        ],
    );
    let workspace = app(root.path());
    let (started, ready) = tokio::sync::oneshot::channel();
    let started = Mutex::new(Some(started));
    let (release, blocked) = std::sync::mpsc::channel();
    let blocked = Mutex::new(blocked);
    let progress = Arc::new(move |_: TraceImportProgress| {
        if let Some(started) = started.lock().unwrap().take() {
            started.send(()).unwrap();
            blocked
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
    });
    let other = workspace.clone();
    let input = request(&path);
    let import = tokio::spawn(async move { other.import_trace(input, progress).await });
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    import.abort();
    assert!(import.await.unwrap_err().is_cancelled());
    {
        let state = workspace.traces.lock();
        assert_eq!(
            state.operations.len(),
            1,
            "worker still holds its bounded slot"
        );
        assert!(state.operations["native-review"].load(Ordering::Acquire));
    }
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !workspace.traces.lock().operations.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_empty(&workspace, ExchangeId(1));
    // The same caller key is reusable after the old worker finishes.
    workspace
        .import_trace(request(&path), Arc::new(|_| {}))
        .await
        .unwrap();
    assert!(workspace.traces.lock().operations.is_empty());
    assert_eq!(
        workspace
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions
            .len(),
        1
    );
}
