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

#[test]
fn native_request_order_uses_saved_ordinals_or_surviving_appearance() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("order.tmcap");
    let heads = [90, 2, 44].map(|id| {
        let mut record = head(format!("https://example.invalid/{id}"));
        record.exchange_id = id;
        record
    });
    write_records(&path, heads.to_vec());
    let imported = read(&path).unwrap();
    assert_eq!(
        imported
            .sessions
            .iter()
            .map(|session| session.request_heads[0].head.target.path.as_str())
            .collect::<Vec<_>>(),
        ["/90", "/2", "/44"]
    );
    let (_, trace, _) =
        TraceRegistry::read(&request(&path), &AtomicBool::new(false), &|_| {}).unwrap();
    assert!(trace.notes.iter().any(|note| note.contains("surviving")));
    let records = heads
        .into_iter()
        .zip([10, 100, 2])
        .flat_map(|(head, order)| {
            let order = CaptureRecord {
                exchange_id: head.exchange_id,
                sequence: 2,
                kind: CaptureRecordKind::Unknown {
                    kind: "request-order".into(),
                    payload: serde_json::json!({"version":1,"ordinal":order.to_string()}),
                },
            };
            [head, order]
        })
        .collect();
    write_records(&path, records);
    let imported = read(&path).unwrap();
    assert_eq!(
        imported
            .sessions
            .iter()
            .map(|session| session.request_heads[0].head.target.path.as_str())
            .collect::<Vec<_>>(),
        ["/44", "/90", "/2"]
    );
}

fn numbered_saz(path: &Path, sessions: &[(u64, u8)]) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for &(id, second) in sessions {
        for (side, text) in [
            (
                "c.txt",
                format!("GET https://example.invalid/source/{id} HTTP/1.1\r\n\r\n"),
            ),
            (
                "s.txt",
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".into(),
            ),
            (
                "m.xml",
                format!(
                    "<Session><SessionTimers ClientBeginRequest='2026-10-14T00:00:{second:02}Z'/></Session>"
                ),
            ),
        ] {
            zip.start_file(
                format!("raw/{id}_{side}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
    }
    zip.finish().unwrap();
}

#[test]
fn zero_source_number_is_preserved_independently_of_positive_workspace_order() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zero.saz");
    numbered_saz(&path, &[(0, 0)]);
    let data = read(&path).unwrap();
    let source = data.entries.into_values().next().unwrap();
    assert_eq!(source.original_number.as_ref().unwrap().value, "0");
    assert!(
        SavedEntry {
            source,
            terminal_unix_millis: None
        }
        .valid()
    );
}

#[tokio::test]
async fn merged_sources_and_saved_subsets_keep_source_numbers_and_relative_order() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("numbered.saz");
    numbered_saz(&path, &[(90, 0), (2, 20), (10, 10)]);
    let workspace = app(root.path());
    for operation in ["first", "second"] {
        let mut input = request(&path);
        input.operation_id = operation.into();
        workspace
            .import_trace(input, Arc::new(|_| {}))
            .await
            .unwrap();
    }
    let rows = workspace
        .query_sessions(crate::SessionQueryInput::default())
        .unwrap()
        .sessions;
    assert_eq!(
        rows.iter()
            .map(|row| row.request_number.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3", "4", "5", "6"]
    );
    assert_eq!(
        rows.iter().map(|row| row.path.as_str()).collect::<Vec<_>>(),
        [
            "/source/2",
            "/source/10",
            "/source/90",
            "/source/2",
            "/source/10",
            "/source/90"
        ]
    );
    assert_ne!(rows[0].trace_id, rows[3].trace_id);
    workspace.session_service().catalog().dismiss(
        &[ExchangeId(u128::from_str_radix(&rows[0].id, 16).unwrap())],
        &[],
    );
    for (format, extension) in [
        (crate::TraceSaveFormat::Native, "tmcap"),
        (crate::TraceSaveFormat::SazExtended, "saz"),
    ] {
        let saved = root.path().join(format!("subset.{extension}"));
        workspace
            .save_traffic_trace(
                saved.clone(),
                crate::TraceSaveOptions {
                    format,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let reopened = app(root.path());
        reopened
            .import_trace(request(&path), Arc::new(|_| {}))
            .await
            .unwrap();
        let mut input = request(&saved);
        input.operation_id = "subset".into();
        reopened
            .import_trace(input, Arc::new(|_| {}))
            .await
            .unwrap();
        let entries = reopened
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions;
        assert_eq!(entries[3].request_number, "4");
        assert_eq!(entries[3].path, "/source/10");
        assert_eq!(entries[4].path, "/source/90");
        assert_eq!(entries[5].path, "/source/2");
        let detail = reopened.session_detail(&entries[3].id).unwrap();
        assert_eq!(detail.original_id.as_deref(), Some("10"));
        assert_eq!(detail.original_request_number.as_deref(), Some("10"));
        assert_eq!(
            detail.original_request_number_provenance.as_deref(),
            Some("SAZ session number")
        );
    }
}

#[test]
fn native_rejects_invalid_stream_tags_and_counts_stream_reason_storage() {
    use transmog_core::stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("stream-metadata.tmcap");
    for sender in [true, false] {
        let mut message = StreamMessage::new("server", "text", bytes::Bytes::new());
        if sender {
            message.sender = "x".repeat(64 * 1024);
        } else {
            message.kind = "x".repeat(64 * 1024);
        }
        write_records(
            &path,
            vec![
                head("https://example.invalid/".into()),
                record(CaptureRecordKind::StreamMessage(message)),
            ],
        );
        assert!(read(&path).is_err());
    }
    let mut charges = Vec::new();
    for size in [0, 64 * 1024] {
        write_records(
            &path,
            vec![
                head("https://example.invalid/".into()),
                record(CaptureRecordKind::StreamState(StreamState::now(
                    StreamProtocol::WebSocket,
                    StreamStatus::Closed,
                    Some("x".repeat(size)),
                ))),
            ],
        );
        let imported = read(&path).unwrap();
        assert_eq!(imported.stream_index_bytes, size);
        charges.push(imported.index_bytes);
    }
    assert_eq!(charges[1] - charges[0], 64 * 1024);
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
    assert!(
        apply_native_frame(
            &mut row,
            frame,
            &SourceReader::new(File::open(&path).unwrap()).unwrap(),
            &mut budget,
            &mut HashSet::new()
        )
        .is_err()
    );
    assert_eq!(
        row.requests,
        [] as [transmog_session::ObservedRequestHead; 0]
    );
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
    assert_eq!(
        app.query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions,
        [] as [crate::SessionSummary; 0]
    );
    assert_eq!(
        app.body_store().unwrap().metadata(id),
        [] as [crate::StoredBodyMetadata; 0]
    );
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

fn header(name: &str, value: &str) -> CapturedHeader {
    CapturedHeader {
        name: name.as_bytes().to_vec(),
        value: Some(value.as_bytes().to_vec()),
        original_value_bytes: None,
    }
}

fn failed() -> CaptureRecord {
    record(CaptureRecordKind::Failed {
        category: "Upstream".into(),
        message: "interrupted".into(),
    })
}

#[test]
fn native_samples_after_a_gap_do_not_become_a_false_contiguous_prefix() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sample-gaps.tmcap");
    for first in [None, Some(b"ab".to_vec())] {
        let expected = first.clone().unwrap_or_default();
        write_records(
            &path,
            vec![
                body(3, first),
                body(2, Some(b"de".to_vec())),
                record(CaptureRecordKind::Completed),
            ],
        );
        let imported = read(&path).unwrap();
        let saved = &imported.bodies[0];
        assert_eq!(saved.observed, 5);
        assert!(!saved.complete);
        assert_eq!(saved.source.length(), Some(expected.len() as u64));
        let mut bytes = Vec::new();
        saved
            .source
            .open()
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(
            bytes, expected,
            "samples on either side of a gap were joined"
        );
    }
}

#[test]
fn native_transfer_encoding_prevents_content_length_completeness_inference() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("framing.tmcap");
    for (length, bytes, finished, expected) in [
        ("0", vec![], false, false),
        ("3", b"abc".to_vec(), false, false),
        ("1", b"abc".to_vec(), true, true),
    ] {
        let mut request = head("https://example.invalid/".into());
        if let CaptureRecordKind::RequestHead { headers, .. } = &mut request.kind {
            *headers = vec![
                header("Content-Length", length),
                header("Transfer-Encoding", "chunked"),
            ];
        }
        let mut records = vec![request, body(bytes.len(), Some(bytes))];
        if finished {
            records.push(record(CaptureRecordKind::Unknown {
                kind: "body-completed".into(),
                payload: serde_json::json!({"boundary":"client-request"}),
            }));
        }
        records.push(failed());
        write_records(&path, records);
        let imported = read(&path).unwrap();
        assert_eq!(
            imported.bodies[0].complete, expected,
            "length={length}, finished={finished}"
        );
    }
}

#[test]
fn native_response_framing_uses_the_request_method_on_its_own_leg() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("head-method.tmcap");
    for client_is_head in [false, true] {
        let mut records = vec![];
        for (boundary, is_head) in [
            (ExchangeBoundary::ClientRequest, client_is_head),
            (ExchangeBoundary::UpstreamRequest, !client_is_head),
        ] {
            records.push(record(CaptureRecordKind::RequestHead {
                boundary,
                method: if is_head { "HEAD" } else { "GET" }.into(),
                target: "https://example.invalid/".into(),
                headers: vec![],
            }));
        }
        for boundary in [
            ExchangeBoundary::ClientResponse,
            ExchangeBoundary::UpstreamResponse,
        ] {
            records.push(record(CaptureRecordKind::ResponseHead {
                boundary,
                status: 200,
                headers: vec![header("Content-Length", "123")],
            }));
        }
        records.push(failed());
        write_records(&path, records);
        let imported = read(&path).unwrap();
        for (boundary, expected) in [
            (ExchangeBoundary::ClientResponse, client_is_head),
            (ExchangeBoundary::UpstreamResponse, !client_is_head),
        ] {
            assert_eq!(
                imported
                    .bodies
                    .iter()
                    .find(|body| body.boundary == boundary)
                    .unwrap()
                    .complete,
                expected,
                "{boundary:?}"
            );
        }
    }
}

#[test]
fn native_terminal_heads_keep_restored_protocols() {
    use transmog_core::performance::{PerformanceEvidence, ProtocolObservation};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("terminal-protocol.tmcap");
    write_records(
        &path,
        vec![
            head("https://example.invalid/".into()),
            record(CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status: 204,
                headers: vec![],
            }),
            record(CaptureRecordKind::Performance(PerformanceEvidence {
                protocols: ["client-request", "client-response"]
                    .map(|boundary| ProtocolObservation {
                        boundary: boundary.into(),
                        version: "HTTP/2".into(),
                        reason: None,
                    })
                    .to_vec(),
                ..Default::default()
            })),
            record(CaptureRecordKind::Completed),
        ],
    );
    let imported = read(&path).unwrap();
    let snapshot = &imported.sessions[0];
    assert_eq!(
        snapshot.request_heads[0].head.source_version,
        HttpLegVersion::Http2
    );
    let Some(SessionTerminal::Completed(terminal)) = &snapshot.terminal else {
        panic!("missing completed exchange")
    };
    assert_eq!(terminal.request_head.source_version, HttpLegVersion::Http2);
    assert_eq!(terminal.response_head.source_version, HttpLegVersion::Http2);
}

#[tokio::test]
async fn native_late_metadata_error_does_not_publish_prior_valid_rows_or_bodies() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("late-error.tmcap");
    let mut bad = record(CaptureRecordKind::Unknown {
        kind: "body-representation".into(),
        payload: serde_json::json!({"boundary":"client-response", "contentCodings":[false], "observedBytes":0, "retainedBytes":0, "complete":true}),
    });
    bad.exchange_id = 2;
    write_records(
        &path,
        vec![
            body(3, Some(b"abc".to_vec())),
            record(CaptureRecordKind::Completed),
            bad,
        ],
    );
    let workspace = app(root.path());
    assert!(
        workspace
            .import_trace(request(&path), Arc::new(|_| {}))
            .await
            .is_err()
    );
    assert_empty(&workspace, ExchangeId(1));
    assert_eq!(workspace.body_store().unwrap().purge_terminal().unwrap(), 0);
}

#[test]
fn native_observer_loss_stops_retaining_a_contiguous_body_prefix() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("observer-gap.tmcap");
    write_records(
        &path,
        vec![
            body(2, Some(b"ab".to_vec())),
            record(CaptureRecordKind::Loss {
                first_missing_sequence: 2,
                count: 1,
                reason: "dropped event".into(),
            }),
            body(2, Some(b"de".to_vec())),
            record(CaptureRecordKind::Completed),
        ],
    );
    let imported = read(&path).unwrap();
    assert_eq!(imported.sessions[0].sequence_loss, 1);
    let saved = &imported.bodies[0];
    assert!(!saved.complete);
    let mut bytes = Vec::new();
    saved
        .source
        .open()
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"ab");
}

#[test]
fn native_misplaced_known_records_are_rejected_instead_of_ignored() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("record-scope.tmcap");
    let mut global_body = body(3, Some(b"abc".to_vec()));
    global_body.exchange_id = 0;
    let mut global_completed = record(CaptureRecordKind::Completed);
    global_completed.exchange_id = 0;
    let global_body_marker = CaptureRecord {
        exchange_id: 0,
        ..record(CaptureRecordKind::Unknown {
            kind: "body-completed".into(),
            payload: serde_json::json!({"boundary":"client-request"}),
        })
    };
    let local_trace_metadata = record(CaptureRecordKind::Unknown {
        kind: "trace-metadata".into(),
        payload: serde_json::json!({}),
    });
    for record in [
        global_body,
        global_completed,
        global_body_marker,
        local_trace_metadata,
    ] {
        write_records(&path, vec![record]);
        assert!(read(&path).is_err(), "accepted misplaced known record");
    }
    // Future extension records remain ignorable in either scope.
    write_records(
        &path,
        [0, 1]
            .map(|exchange_id| CaptureRecord {
                exchange_id,
                ..record(CaptureRecordKind::Unknown {
                    kind: "future-extension".into(),
                    payload: serde_json::json!({}),
                })
            })
            .to_vec(),
    );
    assert!(read(&path).is_ok());
}

#[test]
#[allow(clippy::too_many_lines)]
fn native_import_shared_and_dictionary_bodies_remain_lazy_and_seekable() {
    use std::io::{Read, Seek, SeekFrom, Write};
    use transmog_capture::{CaptureEncoding, CapturePassword};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("paired-bodies.tmcap");
    let mut seed = 0x1234_5678_u32;
    let original: Vec<u8> = (0..128 * 1024)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed.to_le_bytes()[0]
        })
        .collect();
    let mut changed = original.clone();
    changed[70_000..70_100].fill(42);
    for encrypted in [false, true] {
        let password = encrypted.then(|| CapturePassword::new("import test password".into()));
        let options = CaptureEncoding {
            password: password.clone(),
        };
        let mut writer =
            CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options).unwrap();
        for (boundary, bytes) in [
            (ExchangeBoundary::UpstreamResponse, &original),
            (ExchangeBoundary::ClientResponse, &changed),
        ] {
            for chunk in bytes.chunks(64 * 1024) {
                writer
                    .append(&record(CaptureRecordKind::BodySegment {
                        boundary,
                        byte_count: chunk.len(),
                        bytes: Some(chunk.to_vec()),
                        truncated: false,
                    }))
                    .unwrap();
            }
        }
        writer
            .append(&record(CaptureRecordKind::Completed))
            .unwrap();
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap();
        let mut index = CaptureReader::with_password(
            std::io::Cursor::new(&bytes),
            CaptureLimits::default(),
            password.as_ref(),
        )
        .unwrap();
        let mut frames = Vec::new();
        while let Some(frame) = index.read_next_indexed().unwrap() {
            frames.push(frame);
        }
        assert_eq!(
            frames[0].offset, frames[2].offset,
            "first pair must share the object"
        );
        assert!(
            frames[3].frame_bytes < 1024,
            "second pair must use a dictionary"
        );
        std::fs::write(&path, &bytes).unwrap();
        let mut input = request(&path);
        input.password = password;
        let (imported, _, _) =
            TraceRegistry::read(&input, &AtomicBool::new(false), &|_| {}).unwrap();
        for (boundary, expected) in [
            (ExchangeBoundary::UpstreamResponse, &original),
            (ExchangeBoundary::ClientResponse, &changed),
        ] {
            let source = &imported
                .bodies
                .iter()
                .find(|body| body.boundary == boundary)
                .unwrap()
                .source;
            for offset in [0, 65_530, 65_536, 70_010, original.len()] {
                let mut actual = Vec::new();
                source
                    .open_at(offset as u64)
                    .unwrap()
                    .take(512)
                    .read_to_end(&mut actual)
                    .unwrap();
                assert_eq!(&actual, &expected[offset..expected.len().min(offset + 512)]);
            }
            assert!(source.open_at(original.len() as u64 + 1).is_err());
        }
        // Corrupt only the dictionary base, away from the metadata's blocks.
        let corrupt_at = frames[1].offset + 20_000;
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(corrupt_at)).unwrap();
        file.write_all(&[bytes[usize::try_from(corrupt_at).unwrap()] ^ 1])
            .unwrap();
        drop(file);
        // Import must stay lazy even when an unread body is corrupt.
        let (again, _, _) = TraceRegistry::read(&input, &AtomicBool::new(false), &|_| {}).unwrap();
        for data in [&imported, &again] {
            let source = &data
                .bodies
                .iter()
                .find(|body| body.boundary == ExchangeBoundary::ClientResponse)
                .unwrap()
                .source;
            let mut first = Vec::new();
            source
                .open_at(0)
                .unwrap()
                .take(16)
                .read_to_end(&mut first)
                .unwrap();
            assert_eq!(first, changed[..16]);
            assert!(
                source
                    .open_at(70_000)
                    .unwrap()
                    .read_to_end(&mut Vec::new())
                    .is_err()
            );
        }
        drop(imported);
        drop(again);
    }
}

#[test]
fn native_charges_dense_context_allocations() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("dense-context.tmcap");
    let payload = serde_json::json!({"items": vec![Value::Null; 10_000]});
    let minimum = 10_000 * std::mem::size_of::<Value>();
    write_records(
        &path,
        vec![
            CaptureRecord {
                exchange_id: 0,
                sequence: 0,
                kind: CaptureRecordKind::Unknown {
                    kind: "trace-metadata".into(),
                    payload,
                },
            },
            head("https://example.invalid/".into()),
        ],
    );
    assert!(read(&path).unwrap().index_bytes >= minimum);
    let workspace = app(root.path());
    let (data, trace, _) =
        TraceRegistry::read(&request(&path), &AtomicBool::new(false), &|_| {}).unwrap();
    // A serialized-only charge fits; the retained tree does not.
    workspace.traces.lock().metadata_bytes = MAX_HEAD_BYTES - minimum / 2;
    let result = workspace.traces.publish_import(
        data,
        &workspace.service,
        workspace.body_store().unwrap(),
        trace,
        &AtomicBool::new(false),
    );
    assert!(result.is_err());
    assert_eq!(
        workspace.traces.lock().metadata_bytes,
        MAX_HEAD_BYTES - minimum / 2
    );
    workspace.traces.lock().metadata_bytes = 0;
    assert_empty(&workspace, ExchangeId(1));
    let (data, trace, _) =
        TraceRegistry::read(&request(&path), &AtomicBool::new(false), &|_| {}).unwrap();
    let id = data.sessions[0].exchange_id;
    workspace
        .traces
        .publish_import(
            data,
            &workspace.service,
            workspace.body_store().unwrap(),
            trace,
            &AtomicBool::new(false),
        )
        .unwrap();
    let charged = workspace.traces.lock().metadata_bytes;
    assert!(charged >= minimum);
    workspace.traces.forget_entries(&[]);
    // Publishing clones the returned trace; Vec clones may shrink spare
    // capacity. Recalculation may lower the conservative charge, but must
    // continue covering the retained tree rather than only its JSON bytes.
    let recalculated = workspace.traces.lock().metadata_bytes;
    assert!(recalculated >= minimum && recalculated <= charged);
    workspace.traces.forget_entries(&[]);
    assert_eq!(workspace.traces.lock().metadata_bytes, recalculated);
    workspace.traces.forget_entries(&[id]);
    assert_eq!(workspace.traces.lock().metadata_bytes, 0);
}

#[test]
fn native_representation_allocations_are_charged_before_retention() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("representation-budget.tmcap");
    let payload = serde_json::json!({"boundary":"client-request", "contentCodings":[],
        "observedBytes":0, "retainedBytes":0, "complete":true, "extra":vec![Value::Null; 1000]});
    write_records(
        &path,
        vec![record(CaptureRecordKind::Unknown {
            kind: "body-representation".into(),
            payload,
        })],
    );
    let mut capture =
        CaptureReader::new(File::open(&path).unwrap(), CaptureLimits::default()).unwrap();
    let frame = capture.read_next_indexed().unwrap().unwrap();
    let mut row = NativeSession::default();
    let mut budget = MAX_HEAD_BYTES - 10_000;
    assert!(
        apply_native_frame(
            &mut row,
            frame,
            &SourceReader::new(File::open(&path).unwrap()).unwrap(),
            &mut budget,
            &mut HashSet::new()
        )
        .is_err()
    );
    assert!(row.bodies.is_empty());
}

#[test]
fn metadata_budget_includes_sparse_object_node_allocations() {
    // Tiny serialized objects each allocate a full map node, not only a key
    // and value. This shape must not bypass the retained context budget.
    let value = serde_json::json!(
        (0..1000)
            .map(|_| serde_json::json!({"a":null}))
            .collect::<Vec<_>>()
    );
    assert!(json_heap_bytes(&value) >= 1000 * 1024);
}

#[test]
fn imported_stream_is_open_at_capture_and_reads_message_payload_lazily() {
    use transmog_core::stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("stream.tmcap");
    let message = StreamMessage::new("server", "text", b"live payload".to_vec().into());
    let mut records = vec![
        head("https://example.invalid/socket".into()),
        record(CaptureRecordKind::Completed),
        record(CaptureRecordKind::StreamState(StreamState::now(
            StreamProtocol::WebSocket,
            StreamStatus::Open,
            None,
        ))),
        record(CaptureRecordKind::StreamMessage(message.clone())),
    ];
    // File recording and reopening must preserve history beyond the former 1024-item cap.
    records.extend((0..1199).map(|_| record(CaptureRecordKind::StreamMessage(message.clone()))));
    for (index, record) in records.iter_mut().enumerate() {
        record.sequence = index as u64 + 1;
    }
    write_records(&path, records);
    let data = read(&path).unwrap();
    let mut snapshot = data.sessions[0].clone();
    snapshot.imported = true;
    assert_eq!(snapshot.state(), "open-at-capture");
    assert_eq!(snapshot.stream_history.items.len(), 1200);
    let item = &snapshot.stream_history.items[0];
    assert!(item.message.payload.is_empty());
    assert!(item.source.is_some());
    assert_eq!(item.read().unwrap(), message);
}

#[test]
fn imported_stream_diagnostics_do_not_inflate_transfer_counts() {
    use transmog_core::stream::{StreamHistory, StreamMessage};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("stream-diagnostics.tmcap");
    let messages = [
        StreamMessage::new("server", "text", b"hello".to_vec().into()),
        StreamMessage::new("server", "unavailable", b"decoder failed".to_vec().into()),
        StreamMessage::new("client", "incomplete", b"partial frame".to_vec().into()),
    ];
    let mut records = vec![head("https://example.invalid/socket".into())];
    let mut live = StreamHistory::default();
    for (index, message) in messages.into_iter().enumerate() {
        live.push(index as u64 + 2, message.clone());
        let mut entry = record(CaptureRecordKind::StreamMessage(message));
        entry.sequence = index as u64 + 2;
        records.push(entry);
    }
    write_records(&path, records);
    let data = read(&path).unwrap();
    let imported = &data.sessions[0].stream_history;
    assert_eq!(imported.observed, live.observed);
    assert_eq!(imported.client_bytes, live.client_bytes);
    assert_eq!(imported.server_bytes, live.server_bytes);
}

#[test]
fn imported_persistent_sse_ids_share_storage_and_index_charge() {
    use transmog_core::stream::StreamMessage;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("shared-sse-id.tmcap");
    let mut message = StreamMessage::new("server", "heartbeat", b":".to_vec().into());
    message.event_id = Some("x".repeat(64 * 1024).into());
    let mut records = vec![head("https://example.invalid/events".into())];
    records.extend((2..=1201).map(|sequence| CaptureRecord {
        exchange_id: 1,
        sequence,
        kind: CaptureRecordKind::StreamMessage(message.clone()),
    }));
    write_records(&path, records);
    let data = read(&path).unwrap();
    let history = &data.sessions[0].stream_history;
    assert_eq!(history.items.len(), 1200);
    let first = history.items[0].message.event_id.as_ref().unwrap();
    for item in &history.items {
        assert!(Arc::ptr_eq(first, item.message.event_id.as_ref().unwrap()));
    }
    assert!(data.stream_index_bytes < 1200 * 1024 + first.len());
    let loaded = history.items[1199].read().unwrap();
    assert_eq!(loaded, message);
    assert!(Arc::ptr_eq(first, loaded.event_id.as_ref().unwrap()));
}

#[tokio::test]
async fn stream_lifetime_boundaries_survive_native_save_and_reimport() {
    use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
    let root = tempfile::tempdir().unwrap();
    for (index, (protocol, status)) in [StreamProtocol::WebSocket, StreamProtocol::Sse]
        .into_iter()
        .flat_map(|protocol| {
            [
                StreamStatus::Open,
                StreamStatus::Closed,
                StreamStatus::Failed,
                StreamStatus::Interrupted,
            ]
            .map(|status| (protocol, status))
        })
        .enumerate()
    {
        let input = root.path().join(format!("lifetime-{index}.tmcap"));
        let mut records = vec![head("https://example.invalid/socket".into())];
        for (status, at_millis) in [
            (StreamStatus::Connecting, 1000),
            (StreamStatus::Open, 2000),
            (status, 8000),
        ] {
            records.push(record(CaptureRecordKind::StreamState(StreamState {
                protocol,
                status,
                at_millis,
                reason: None,
            })));
        }
        records.push(record(CaptureRecordKind::Unknown {kind:"stream-summary".into(),payload:serde_json::json!({"openedAt":1000,"establishedAt":2000,"endedAt":if status.is_active() {None} else {Some(8000)},"observedUntil":8000,"elapsedMillis":6000})}));
        for (index, record) in records.iter_mut().enumerate() {
            record.sequence = index as u64 + 1;
        }
        write_records(&input, records);
        let workspace = app(root.path());
        workspace
            .import_trace(request(&input), Arc::new(|_| {}))
            .await
            .unwrap();
        let row = workspace
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions
            .remove(0);
        assert_eq!(row.duration_ms, Some(6000));
        let detail = workspace.session_detail(&row.id).unwrap();
        let stream = detail.stream.unwrap();
        assert_eq!(stream.lifetime.established_at, Some(2000));
        assert_eq!(stream.opened_at, Some(1000));
        assert_eq!(stream.lifetime.observed_until, Some(8000));
        assert!(!stream.lifetime.live);
        let saved = root.path().join(format!("saved-lifetime-{index}.tmcap"));
        workspace
            .save_traffic_trace(
                saved.clone(),
                crate::TraceSaveOptions {
                    format: crate::TraceSaveFormat::Native,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let saved_data = read(&saved).unwrap();
        let snapshot = &saved_data.sessions[0];
        assert_eq!(snapshot.stream_history.established_at, Some(2000));
        assert_eq!(snapshot.stream_history.opened_at, Some(1000));
        assert_eq!(snapshot.stream_history.elapsed_millis, Some(6000));
        assert_eq!(snapshot.stream_history.observed_until, Some(8000));
        assert_eq!(
            snapshot.stream_history.ended_at,
            if status.is_active() { None } else { Some(8000) }
        );
    }
}

#[test]
fn old_stream_summary_first_observation_is_not_relabelled_as_establishment() {
    use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("old-summary.tmcap");
    let mut records = vec![
        head("https://example.invalid/socket".into()),
        record(CaptureRecordKind::StreamState(StreamState {
            protocol: StreamProtocol::WebSocket,
            status: StreamStatus::Closed,
            at_millis: 8000,
            reason: None,
        })),
        record(CaptureRecordKind::Unknown {
            kind: "stream-summary".into(),
            payload: serde_json::json!({"openedAt":1000,"lastActivity":7000}),
        }),
    ];
    for (index, record) in records.iter_mut().enumerate() {
        record.sequence = index as u64 + 1;
    }
    write_records(&path, records);
    let data = read(&path).unwrap();
    let snapshot = &data.sessions[0];
    assert_eq!(snapshot.stream_history.opened_at, Some(1000));
    assert_eq!(snapshot.stream_history.established_at, None);
    assert_eq!(snapshot.stream_history.lifetime_millis(), None);
}

#[tokio::test]
async fn sse_http_terminals_use_recorded_end_time_and_survive_save_reimport() {
    use transmog_core::performance::{Milestone, PerformanceEvidence, TimingPoint};
    use transmog_core::stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus};
    let root = tempfile::tempdir().unwrap();
    for (index, (category, expected)) in [
        (None, StreamStatus::Closed),
        (Some("Upstream"), StreamStatus::Failed),
        (Some("Cancelled"), StreamStatus::Interrupted),
        (Some("Shutdown"), StreamStatus::Interrupted),
    ]
    .into_iter()
    .enumerate()
    {
        let path = root.path().join(format!("sse-terminal-{index}.tmcap"));
        let mut message = StreamMessage::new("server", "event", b"quiet".to_vec().into());
        message.at_millis = 4000;
        let mut records = vec![
            head("https://example.invalid/events".into()),
            record(CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status: 200,
                headers: vec![header("Content-Type", "text/event-stream")],
            }),
            record(CaptureRecordKind::StreamState(StreamState {
                protocol: StreamProtocol::Sse,
                status: StreamStatus::Open,
                at_millis: 2000,
                reason: None,
            })),
            record(CaptureRecordKind::StreamMessage(message)),
            record(CaptureRecordKind::Performance(PerformanceEvidence {
                points: vec![
                    TimingPoint {
                        milestone: Milestone::RequestHeaders,
                        unix_millis: 1000,
                        offset_micros: 0,
                    },
                    TimingPoint {
                        milestone: Milestone::ExchangeDone,
                        unix_millis: 8000,
                        offset_micros: 7_000_000,
                    },
                ],
                ..Default::default()
            })),
            record(category.map_or(CaptureRecordKind::Completed, |category| {
                CaptureRecordKind::Failed {
                    category: category.into(),
                    message: "source terminal".into(),
                }
            })),
        ];
        for (index, record) in records.iter_mut().enumerate() {
            record.sequence = index as u64 + 1;
        }
        write_records(&path, records);
        let workspace = app(root.path());
        workspace
            .import_trace(request(&path), Arc::new(|_| {}))
            .await
            .unwrap();
        let row = workspace
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions
            .remove(0);
        let stream = workspace.session_detail(&row.id).unwrap().stream.unwrap();
        assert_eq!(stream.state.status, expected);
        assert_eq!(stream.state.at_millis, 8000);
        assert_eq!(stream.lifetime.ended_at, Some(8000));
        assert_eq!(stream.lifetime.duration_ms, Some(6000));
        assert_eq!(stream.last_activity, Some(4000));
        assert_eq!(stream.lifetime.boundary, "final");
        let saved = root.path().join(format!("sse-resaved-{index}.tmcap"));
        workspace
            .save_traffic_trace(
                saved.clone(),
                crate::TraceSaveOptions {
                    format: crate::TraceSaveFormat::Native,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let data = read(&saved).unwrap();
        let snapshot = &data.sessions[0];
        assert_eq!(snapshot.stream.as_ref().unwrap().status, expected);
        assert_eq!(snapshot.stream_history.ended_at, Some(8000));
        assert_eq!(snapshot.stream_history.elapsed_millis, Some(6000));
    }
}

#[test]
fn native_sse_incomplete_capture_is_open_and_missing_terminal_time_is_not_invented() {
    use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
    let root = tempfile::tempdir().unwrap();
    for (name, terminal) in [
        ("missing-end", CaptureRecordKind::Completed),
        (
            "open-at-capture",
            CaptureRecordKind::Failed {
                category: "incomplete".into(),
                message: "saved while open".into(),
            },
        ),
    ] {
        let path = root.path().join(format!("{name}.tmcap"));
        let mut records = vec![
            head("https://example.invalid/events".into()),
            record(CaptureRecordKind::StreamState(StreamState {
                protocol: StreamProtocol::Sse,
                status: StreamStatus::Open,
                at_millis: 2000,
                reason: None,
            })),
            record(terminal),
        ];
        for (index, record) in records.iter_mut().enumerate() {
            record.sequence = index as u64 + 1;
        }
        write_records(&path, records);
        let data = read(&path).unwrap();
        let snapshot = &data.sessions[0];
        assert_eq!(snapshot.stream_history.ended_at, None);
        assert_eq!(snapshot.stream_history.elapsed_millis, None);
        if name == "missing-end" {
            assert_eq!(
                snapshot.stream.as_ref().unwrap().status,
                StreamStatus::Closed
            );
            assert_eq!(snapshot.stream.as_ref().unwrap().at_millis, 0);
        } else {
            assert_eq!(snapshot.state(), "open-at-capture");
            assert_eq!(snapshot.stream.as_ref().unwrap().at_millis, 2000);
        }
    }
}

#[tokio::test]
async fn stream_counter_availability_survives_save_and_reimport() {
    use transmog_core::stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus};
    let root = tempfile::tempdir().unwrap();
    for (index, (summary, message, expected)) in [
        (None, None, (None, None)),
        (Some(serde_json::json!({"observed": 1})), None, (None, None)),
        (
            Some(serde_json::json!({"clientBytes": 0, "serverBytes": null})),
            None,
            (Some(0), None),
        ),
        (
            Some(serde_json::json!({"serverBytes": 7})),
            None,
            (None, Some(7)),
        ),
        (
            None,
            Some(StreamMessage::new("client", "text", bytes::Bytes::new())),
            (Some(0), None),
        ),
        (
            None,
            Some(StreamMessage::new("server", "text", b"hi".to_vec().into())),
            (None, Some(2)),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let input = root.path().join(format!("counter-{index}.tmcap"));
        let mut records = vec![
            head("https://example.invalid/socket".into()),
            record(CaptureRecordKind::StreamState(StreamState {
                protocol: StreamProtocol::WebSocket,
                status: StreamStatus::Closed,
                at_millis: 8000,
                reason: None,
            })),
        ];
        if let Some(payload) = summary {
            records.push(record(CaptureRecordKind::Unknown {
                kind: "stream-summary".into(),
                payload,
            }));
        }
        if let Some(message) = message {
            records.push(record(CaptureRecordKind::StreamMessage(message)));
        }
        for (sequence, record) in records.iter_mut().enumerate() {
            record.sequence = sequence as u64 + 1;
        }
        write_records(&input, records);
        let case = root.path().join(format!("case-{index}"));
        std::fs::create_dir(&case).unwrap();
        let workspace = app(&case);
        workspace
            .import_trace(request(&input), Arc::new(|_| {}))
            .await
            .unwrap();
        let row = workspace
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions
            .remove(0);
        assert_eq!((row.stream_sent, row.stream_received), expected);
        let detail = workspace.session_detail(&row.id).unwrap().stream.unwrap();
        assert_eq!((detail.client_bytes, detail.server_bytes), expected);
        let saved = root.path().join(format!("saved-counter-{index}.tmcap"));
        workspace
            .save_traffic_trace(
                saved.clone(),
                crate::TraceSaveOptions {
                    format: crate::TraceSaveFormat::Native,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let data = read(&saved).unwrap();
        let history = &data.sessions[0].stream_history;
        assert_eq!(
            (
                history.client_bytes_known.then_some(history.client_bytes),
                history.server_bytes_known.then_some(history.server_bytes)
            ),
            expected
        );
    }
}
