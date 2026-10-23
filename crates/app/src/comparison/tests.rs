use super::*;
use transmog_core::{HeaderBlock, HeaderField};

fn evidence(headers: HeaderBlock) -> Evidence {
    Evidence {
        side: ComparisonSide {
            label: "Fixture".into(),
            source: "Synthetic".into(),
            entry_id: Some("1".into()),
            acquired_at: 0,
            boundary: "client-response".into(),
            notes: vec![],
        },
        request: None,
        response: Some(Message {
            boundary: "client-response".into(),
            first: "200".into(),
            protocol: None,
            headers,
            body: Body::missing("not retained"),
            available: true,
            trailers: None,
        }),
        outcome: None,
        timings: vec![],
        versions: vec![],
        omissions: vec![],
        stream: None,
        stream_indicated: false,
        exchange: true,
    }
}
fn headers(fields: &[(&str, &str)]) -> HeaderBlock {
    HeaderBlock::from_fields(
        fields
            .iter()
            .map(|(n, v)| HeaderField::try_new(*n, *v).unwrap())
            .collect(),
    )
}
#[tokio::test]
async fn compares_beyond_inspector_display_caps_and_keeps_partial_coverage() {
    let mut a = (0..600)
        .map(|i| HeaderField::try_new(format!("x-{i}"), "same").unwrap())
        .collect::<Vec<_>>();
    let mut b = a.clone();
    a.push(HeaderField::try_new("x-long", format!("{}a", "x".repeat(3000))).unwrap());
    b.push(HeaderField::try_new("x-long", format!("{}b", "x".repeat(3000))).unwrap());
    let registry = ComparisonRegistry::default();
    let op = registry.begin("test").unwrap();
    let (rows, _, _) = algorithm::compare(
        &evidence(HeaderBlock::from_fields(a)),
        &evidence(HeaderBlock::from_fields(b)),
        &ComparisonOptions::default(),
        &op,
    )
    .await
    .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.field.contains("x-long") && r.state == ComparisonState::Different)
    );
    assert!(
        rows.iter()
            .any(|r| r.field == "Body" && r.state == ComparisonState::Incomplete)
    );
}
#[tokio::test]
async fn duplicate_headers_keep_spelling_changes_and_explicit_exclusions() {
    let a = evidence(headers(&[
        ("Set-Cookie", "a=1"),
        ("Set-Cookie", "b=2"),
        ("Date", "Monday"),
    ]));
    let b = evidence(headers(&[
        ("set-cookie", "b=2"),
        ("Set-Cookie", "a=1"),
        ("Date", "Tuesday"),
    ]));
    let registry = ComparisonRegistry::default();
    let op = registry.begin("test").unwrap();
    let (rows, _, _) = algorithm::compare(
        &a,
        &b,
        &ComparisonOptions {
            excluded_headers: vec!["date".into()],
            normalize_newlines: false,
        },
        &op,
    )
    .await
    .unwrap();
    assert_eq!(
        rows.iter()
            .filter(|r| r.field.contains("set-cookie") && r.state == ComparisonState::Different)
            .count(),
        1
    );
    let spelling = rows
        .iter()
        .find(|r| r.field.contains("set-cookie") && r.state == ComparisonState::Different)
        .unwrap();
    assert_eq!(spelling.left, "Set-Cookie: b=2");
    assert_eq!(spelling.right, "set-cookie: b=2");
    assert_eq!(
        spelling.note,
        "Original field-name spelling changed; value unchanged"
    );
    assert!(
        rows.iter()
            .any(|r| r.state == ComparisonState::Excluded && r.field.contains("date"))
    );
}
#[tokio::test]
async fn reordered_identical_headers_and_duplicates_are_equal() {
    let a = evidence(headers(&[
        ("X-Repeat", "same"),
        ("Accept", "text/plain"),
        ("x-repeat", "same"),
        ("X-Repeat", "distinct"),
        ("X-Repeat", "same"),
    ]));
    let b = evidence(headers(&[
        ("x-repeat", "same"),
        ("X-Repeat", "distinct"),
        ("X-Repeat", "same"),
        ("X-Repeat", "same"),
        ("Accept", "text/plain"),
    ]));
    for (left, right) in [(&a, &b), (&b, &a)] {
        let registry = ComparisonRegistry::default();
        let op = registry.begin("reordered").unwrap();
        let (rows, _, tally) = algorithm::compare(left, right, &ComparisonOptions::default(), &op)
            .await
            .unwrap();
        assert_eq!(tally.changed, 0);
        assert!(
            !rows
                .iter()
                .any(|r| r.field.starts_with("Header ") && r.field != "Header summary")
        );
        assert!(
            rows.iter()
                .any(|r| r.field == "Header summary" && r.state == ComparisonState::Equal)
        );
    }
}
#[tokio::test]
async fn added_or_removed_duplicates_do_not_flag_shifted_headers() {
    let a = evidence(headers(&[("X-Repeat", "same"), ("Accept", "text/plain")]));
    let b = evidence(headers(&[
        ("X-Repeat", "same"),
        ("X-Repeat", "same"),
        ("Accept", "text/plain"),
    ]));
    for (left, right) in [(&a, &b), (&b, &a)] {
        let registry = ComparisonRegistry::default();
        let op = registry.begin("duplicate-count").unwrap();
        let (rows, _, _) = algorithm::compare(left, right, &ComparisonOptions::default(), &op)
            .await
            .unwrap();
        let changes = rows
            .iter()
            .filter(|r| r.field.starts_with("Header x-repeat"))
            .collect::<Vec<_>>();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].state, ComparisonState::Different);
        assert!(changes[0].left == "Absent" || changes[0].right == "Absent");
        assert!(!rows.iter().any(|r| r.field.contains("accept")));
        assert!(
            rows.iter()
                .any(|r| r.field == "Header summary" && r.state == ComparisonState::Different)
        );
    }
}
#[tokio::test]
async fn reordered_headers_still_show_changed_values() {
    let a = evidence(headers(&[
        ("Accept", "text/plain"),
        ("X-Repeat", "before"),
        ("X-Repeat", "same"),
    ]));
    let b = evidence(headers(&[
        ("X-Repeat", "same"),
        ("X-Repeat", "after"),
        ("Accept", "text/plain"),
    ]));
    let registry = ComparisonRegistry::default();
    let op = registry.begin("changed-value").unwrap();
    let (rows, _, _) = algorithm::compare(&a, &b, &ComparisonOptions::default(), &op)
        .await
        .unwrap();
    let changes = rows
        .iter()
        .filter(|r| r.field.starts_with("Header x-repeat"))
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].state, ComparisonState::Different);
    assert_eq!(changes[0].left, "X-Repeat: before");
    assert_eq!(changes[0].right, "X-Repeat: after");
    assert!(!rows.iter().any(|r| r.field.contains("accept")));
}
#[test]
fn url_pairing_preserves_path_query_and_encoded_separators() {
    let key = pairing::normalize_url(
        "HTTPS://EXAMPLE.test:443/Path%2fPart?a=1&token=x&a=2&token=y",
        &["token".into()],
    )
    .unwrap();
    assert_eq!(key, "https://example.test/Path%2fPart?a=1&a=2");
    assert_ne!(
        pairing::normalize_url("https://x.test/a?", &[]),
        pairing::normalize_url("https://x.test/a", &[])
    );
    assert_ne!(
        pairing::normalize_url("https://x.test/a/../b", &[]),
        pairing::normalize_url("https://x.test/b", &[])
    );
}
#[test]
fn operation_limit_and_cancellation_preserve_bounded_workers() {
    let registry = ComparisonRegistry::default();
    let a = registry.begin("a").unwrap();
    let b = registry.begin("b").unwrap();
    assert!(registry.begin("c").is_err());
    a.canceled.store(true, Ordering::Release);
    assert!(a.check().is_err());
    drop(a);
    assert!(registry.begin("c").is_ok());
    drop(b);
}

#[tokio::test]
async fn redacted_values_never_prove_equal_and_zero_timings_are_measured() {
    let mut field = HeaderField::try_new("Authorization", "secret").unwrap();
    field.redact_value();
    let mut left = evidence(HeaderBlock::from_fields(vec![field.clone()]));
    let mut right = evidence(HeaderBlock::from_fields(vec![field]));
    left.timings.push(("Measured interval".into(), 0));
    right.timings.push(("Measured interval".into(), 5));
    let registry = ComparisonRegistry::default();
    let op = registry.begin("redacted").unwrap();
    let (rows, _, _) = algorithm::compare(&left, &right, &ComparisonOptions::default(), &op)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.field.contains("authorization") && r.state == ComparisonState::Unavailable)
    );
    assert!(
        rows.iter().any(|r| r.field == "Measured interval"
            && r.left == "0 µs"
            && r.note.contains("Delta 5"))
    );
}

fn saz(path: &std::path::Path, responses: &[&str]) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (index, body) in responses.iter().enumerate() {
        let id = index + 1;
        for (name, text) in [
            (
                format!("raw/{id}_c.txt"),
                "GET https://compare.invalid/retry HTTP/1.1\r\nHost: compare.invalid\r\n\r\n"
                    .into(),
            ),
            (
                format!("raw/{id}_s.txt"),
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\r\n{body}",
                    body.len()
                ),
            ),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
    }
    zip.finish().unwrap();
}
fn app(root: &std::path::Path) -> Application {
    Application::new(crate::AppConfig {
        body_store: Some(crate::BodyStoreConfig::product_default(root.join("cache"))),
        response_asset_root: Some(root.join("assets")),
        ..crate::AppConfig::default()
    })
    .unwrap()
}
async fn import(app: &Application, path: std::path::PathBuf, operation: &str) -> String {
    app.import_trace(
        crate::TraceImportRequest {
            path,
            password: None,
            operation_id: operation.into(),
            max_file_bytes: 1024 * 1024,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap()
    .trace
    .id
}
fn request(left: ComparisonSubject, right: ComparisonSubject) -> ComparisonRequest {
    ComparisonRequest {
        operation_id: token().unwrap(),
        left,
        right,
        options: ComparisonOptions::default(),
    }
}
fn exchange(id: &str) -> ComparisonSubject {
    ComparisonSubject::Exchange {
        id: id.into(),
        request_boundary: request_boundary(),
        response_boundary: response_boundary(),
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One frozen investigation exercises correction, ordering, refresh and expiry.
async fn imported_pairs_freeze_bytes_and_keep_repeated_occurrences_uncertain() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.saz");
    let other = root.path().join("b.saz");
    saz(
        &path,
        &["<h1>before</h1>", "<h1>middle</h1>", "<h1>after</h1>"],
    );
    saz(&other, &["<h1>before</h1>", "<h1>after</h1>"]);
    let app = app(root.path());
    let left = import(&app, path, "left").await;
    let right = import(&app, other, "right").await;
    let capture = app
        .compare_captures(
            "pair".into(),
            ComparisonScope::Source {
                trace_id: Some(left),
            },
            ComparisonScope::Source {
                trace_id: Some(right),
            },
            PairingOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        (capture.left_count, capture.right_count, capture.unresolved),
        (3, 2, 3)
    );
    assert_eq!(capture.equal_metadata, 0);
    let pair = &capture.rows[0];
    let a = pair.left.clone().unwrap();
    let b = pair.right.clone().unwrap();
    let reversed = app
        .compare_captures(
            "reversed".into(),
            ComparisonScope::Selected {
                ids: capture
                    .rows
                    .iter()
                    .filter_map(|r| r.left.clone())
                    .rev()
                    .collect(),
            },
            ComparisonScope::Selected {
                ids: capture
                    .rows
                    .iter()
                    .filter_map(|r| r.right.clone())
                    .rev()
                    .collect(),
            },
            PairingOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(reversed.rows[0].left.as_ref(), Some(&a));
    assert_eq!(reversed.rows[0].right.as_ref(), Some(&b));
    assert!(reversed.left_details.contains("saz: 3"));
    assert!(reversed.left_details.contains("unavailable"));
    app.close_capture_comparison(&reversed.id);
    let fixed = app
        .capture_comparison_page(
            &capture.id,
            0,
            None,
            Some(a.clone()),
            Some(b.clone()),
            "pair",
        )
        .unwrap();
    assert!(
        fixed
            .rows
            .iter()
            .any(|r| r.left.as_deref() == Some(&a) && r.reason == "Explicit user choice")
    );
    let filtered = app
        .capture_comparison_page(
            &capture.id,
            0,
            Some(PairingOptions {
                excluded_query_names: vec!["timestamp".into()],
                ..PairingOptions::default()
            }),
            None,
            None,
            "",
        )
        .unwrap();
    assert!(
        filtered
            .rows
            .iter()
            .any(|r| r.left.as_deref() == Some(&a) && r.reason == "Explicit user choice")
    );
    let undone = app
        .capture_comparison_page(&capture.id, 0, None, None, None, "undo")
        .unwrap();
    assert_eq!(undone.unresolved, 3);
    let result = app
        .compare_evidence(request(exchange(&a), exchange(&b)), [None, None])
        .await
        .unwrap();
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Decoded bytes" && r.state == ComparisonState::Equal)
    );
    let bytes = app.comparison_bytes(&result.id, "Response", 0).unwrap();
    assert!(bytes.left.contains("before"));
    app.remove_traffic_entries(std::slice::from_ref(&a), false)
        .await
        .unwrap();
    assert!(app.comparison_stale(&result.id).unwrap());
    assert_eq!(
        app.comparison_bytes(&result.id, "Response", 0)
            .unwrap()
            .left,
        bytes.left
    );
    app.close_comparison(&result.id);
    assert!(app.comparison_page(&result.id, 0).is_err());
}

#[tokio::test]
async fn draft_and_preview_are_read_only_and_expired_observations_are_not_empty_http() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("preview.saz");
    saz(&path, &["<h1>before</h1>"]);
    let app = app(root.path());
    import(&app, path, "preview").await;
    let id = app
        .query_sessions(crate::SessionQueryInput::default())
        .unwrap()
        .sessions[0]
        .id
        .clone();
    let scene = app
        .prepare_captured_page(id.clone(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    scene.resolve("GET", &scene.url, None, None, true).unwrap();
    let diagnostics = scene.diagnostics();
    let before = diagnostics.snapshot();
    let resource = diagnostics
        .supplied_response(before.requests.last().unwrap().id)
        .unwrap();
    let preview = ComparisonSubject::Preview {
        run: "run".into(),
        observation: 1,
    };
    let result = app
        .compare_evidence(
            request(exchange(&id), preview.clone()),
            [
                None,
                Some(PreviewComparisonEvidence {
                    resource,
                    source_id: Some(id.clone()),
                    label: "Frozen run".into(),
                    notes: vec!["Frozen rule Example @2 · asset example@3".into()],
                }),
            ],
        )
        .await
        .unwrap();
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Decoded bytes" && r.state == ComparisonState::Equal)
    );
    assert!(result.rows.iter().any(|r| r.note.contains("example@3")));
    assert_eq!(diagnostics.snapshot().hits, before.hits);
    let missing = app
        .compare_evidence(request(preview.clone(), preview), [None, None])
        .await
        .unwrap();
    assert!(!missing.complete);
    assert!(
        missing
            .rows
            .iter()
            .any(|r| r.section == "Response" && r.state == ComparisonState::Unavailable)
    );
    let assets_before = app.response_assets().len();
    let draft = ComparisonSubject::Draft {
        label: "Unsaved".into(),
        status: "invalid status".into(),
        headers: headers(&[("Content-Type", "text/plain")]),
        decoded_body: Some(b"changed".to_vec()),
        base: Some(Box::new(exchange(&id))),
        body_path: None,
    };
    let result = app
        .compare_evidence(request(exchange(&id), draft), [None, None])
        .await
        .unwrap();
    assert!(result.different);
    assert_eq!(app.response_assets().len(), assets_before);
    assert!(
        result
            .right
            .notes
            .iter()
            .any(|n| n.contains("Invalid draft status"))
    );
}

#[tokio::test]
async fn linked_asset_comparison_keeps_old_bytes_after_external_edit() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("linked.bin");
    std::fs::write(&file, vec![42u8; 25_000_001]).unwrap();
    let app = app(root.path());
    let asset = app
        .import_response_asset(crate::ImportResponseAsset {
            id: "linked".into(),
            revision: 1,
            status: 404,
            headers: headers(&[("Content-Type", "application/octet-stream")]),
            body_path: file.clone(),
            body_file_mode: crate::ResponseBodyFileMode::Linked,
            media_type: None,
        })
        .unwrap();
    assert!(asset.linked_body.is_some());
    let subject = ComparisonSubject::Asset {
        reference: asset.asset_ref(),
    };
    let result = app
        .compare_evidence(request(subject.clone(), subject.clone()), [None, None])
        .await
        .unwrap();
    let before = app.comparison_bytes(&result.id, "Response", 0).unwrap();
    std::fs::write(&file, b"later").unwrap();
    assert_eq!(
        before.left,
        app.comparison_bytes(&result.id, "Response", 0)
            .unwrap()
            .left
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Status" && r.left == "404" && r.state == ComparisonState::Equal)
    );
    let later = app
        .compare_evidence(request(subject.clone(), subject), [None, None])
        .await
        .unwrap();
    assert_ne!(
        before.left,
        app.comparison_bytes(&later.id, "Response", 0).unwrap().left
    );
}

#[tokio::test]
async fn representation_and_prefix_verdicts_do_not_claim_full_equality() {
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let left_path = root.path().join("left");
    let right_path = root.path().join("right");
    let set_body =
        |evidence: &mut Evidence, path: &std::path::Path, bytes: &[u8], complete: bool| {
            std::fs::write(path, bytes).unwrap();
            evidence.response.as_mut().unwrap().body = Body {
                path: Some(path.into()),
                length: Some(bytes.len() as u64),
                digest: Some(hex::encode(Sha256::digest(bytes))),
                complete,
                representation: "encoded entity bytes".into(),
                codings: vec![],
                charset: None,
                reason: if complete {
                    String::new()
                } else {
                    "Truncated".into()
                },
            };
        };
    let registry = ComparisonRegistry::default();
    let op = registry.begin("representations").unwrap();
    let mut left = evidence(headers(&[]));
    let mut right = evidence(headers(&[]));
    set_body(&mut left, &left_path, b"same\r\n", true);
    set_body(&mut right, &right_path, b"same\n", true);
    let (rows, _, _) = algorithm::compare(
        &left,
        &right,
        &ComparisonOptions {
            normalize_newlines: true,
            ..ComparisonOptions::default()
        },
        &op,
    )
    .await
    .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.field == "Decoded bytes" && r.state == ComparisonState::Different)
    );
    assert!(
        rows.iter()
            .any(|r| r.field == "Decoded text" && r.state == ComparisonState::Equal)
    );
    assert_eq!(std::fs::read(&left_path).unwrap(), b"same\r\n");
    set_body(&mut left, &left_path, b"prefix", false);
    set_body(&mut right, &right_path, b"prefix-extra", false);
    let (rows, _, tally) = algorithm::compare(&left, &right, &ComparisonOptions::default(), &op)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.field == "Body" && r.state == ComparisonState::Incomplete)
    );
    assert!(
        rows.iter()
            .any(|r| r.field == "Retained prefix bytes (partial)"
                && r.state == ComparisonState::Equal)
    );
    assert!(tally.unknown > 0);
    set_body(&mut left, &left_path, &[0xff, 0x00], true);
    set_body(&mut right, &right_path, &[0xfe, 0x00], true);
    let (rows, _, _) = algorithm::compare(&left, &right, &ComparisonOptions::default(), &op)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.field == "Text body" && r.state == ComparisonState::NotApplicable)
    );
    assert!(
        rows.iter().any(
            |r| r.field == "Exact body representation" && r.state == ComparisonState::Different
        )
    );
}

#[tokio::test]
async fn oversized_and_missing_linked_assets_keep_status_and_prefix_evidence() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("large.bin");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(40 * 1024 * 1024)
        .unwrap();
    let app = app(root.path());
    let asset = app
        .import_response_asset(crate::ImportResponseAsset {
            id: "large".into(),
            revision: 1,
            status: 404,
            headers: headers(&[("Content-Type", "application/octet-stream")]),
            body_path: source.clone(),
            body_file_mode: crate::ResponseBodyFileMode::Linked,
            media_type: None,
        })
        .unwrap();
    let subject = ComparisonSubject::Asset {
        reference: asset.asset_ref(),
    };
    let result = app
        .compare_evidence(request(subject.clone(), subject.clone()), [None, None])
        .await
        .unwrap();
    assert!(!result.complete);
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Status" && r.state == ComparisonState::Equal && r.left == "404")
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Retained prefix bytes (partial)"
                && r.state == ComparisonState::Equal)
    );
    let before = app.comparison_bytes(&result.id, "Response", 0).unwrap();
    std::fs::remove_file(source).unwrap();
    let missing = app
        .compare_evidence(request(subject.clone(), subject), [None, None])
        .await
        .unwrap();
    assert!(
        missing
            .rows
            .iter()
            .any(|r| r.field == "Status" && r.state == ComparisonState::Equal)
    );
    assert!(
        missing
            .rows
            .iter()
            .any(|r| r.field == "Body" && r.state == ComparisonState::Incomplete)
    );
    assert_eq!(
        before.left,
        app.comparison_bytes(&result.id, "Response", 0)
            .unwrap()
            .left
    );
}

#[tokio::test]
async fn copied_assets_keep_exact_encoding_separate_from_decoded_equality() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let text = b"same decoded content".to_vec();
    let encoded = crate::inspector::encode_content(&["gzip".into()], text.clone())
        .await
        .unwrap();
    let compressed = app
        .create_response_asset(crate::AuthoredResponseAsset {
            id: "compressed".into(),
            revision: 1,
            status: 200,
            headers: headers(&[("Content-Type", "text/plain"), ("Content-Encoding", "gzip")]),
            body: encoded,
            media_type: None,
        })
        .unwrap();
    let plain = app
        .create_response_asset(crate::AuthoredResponseAsset {
            id: "plain".into(),
            revision: 1,
            status: 200,
            headers: headers(&[("Content-Type", "text/plain")]),
            body: text,
            media_type: None,
        })
        .unwrap();
    let result = app
        .compare_evidence(
            request(
                ComparisonSubject::Asset {
                    reference: compressed.asset_ref(),
                },
                ComparisonSubject::Asset {
                    reference: plain.asset_ref(),
                },
            ),
            [None, None],
        )
        .await
        .unwrap();
    assert!(
        result.rows.iter().any(
            |r| r.field == "Exact body representation" && r.state == ComparisonState::Different
        )
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Decoded bytes" && r.state == ComparisonState::Equal)
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.section == "Request" && r.state == ComparisonState::NotApplicable)
    );
}

#[tokio::test]
async fn informational_provenance_is_not_missing_evidence_and_invalid_drafts_stay_unknown() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let asset = app
        .create_response_asset(crate::AuthoredResponseAsset {
            id: "complete".into(),
            revision: 1,
            status: 200,
            headers: headers(&[("Content-Type", "text/plain")]),
            body: b"complete".to_vec(),
            media_type: None,
        })
        .unwrap();
    let subject = ComparisonSubject::Asset {
        reference: asset.asset_ref(),
    };
    let equal = app
        .compare_evidence(request(subject.clone(), subject), [None, None])
        .await
        .unwrap();
    assert!(equal.complete);
    assert!(!equal.different);
    assert_eq!(equal.unknown, 0);
    let draft = ComparisonSubject::Draft {
        label: "Invalid proposal".into(),
        status: "not a status".into(),
        headers: headers(&[]),
        decoded_body: Some(vec![]),
        base: None,
        body_path: None,
    };
    let invalid = app
        .compare_evidence(request(draft.clone(), draft), [None, None])
        .await
        .unwrap();
    assert!(!invalid.complete);
    assert!(invalid.rows.iter().any(|r| r.field == "Status"
        && r.state == ComparisonState::Unavailable
        && r.left == "not a status"));
}

#[tokio::test]
async fn har_decoded_bytes_do_not_claim_original_encoded_wire_equality() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let path = root.path().join("decoded.har");
    let document = serde_json::json!({"log":{"version":"1.2","entries":[{"startedDateTime":"2026-10-15T00:00:00Z","time":1,"request":{"method":"GET","url":"https://har.test/","httpVersion":"HTTP/2","headers":[],"bodySize":0},"response":{"status":200,"httpVersion":"HTTP/2","headers":[{"name":"Content-Type","value":"text/plain; charset=utf-8"},{"name":"Content-Encoding","value":"gzip"}],"bodySize":28,"content":{"text":"decoded","size":7,"mimeType":"text/plain"}},"timings":{"send":0,"wait":1,"receive":0}}]}});
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    import(&app, path, "har").await;
    let id = app
        .query_sessions(crate::SessionQueryInput::default())
        .unwrap()
        .sessions[0]
        .id
        .clone();
    let encoded = crate::inspector::encode_content(&["gzip".into()], b"decoded".to_vec())
        .await
        .unwrap();
    let asset = app
        .create_response_asset(crate::AuthoredResponseAsset {
            id: "encoded".into(),
            revision: 1,
            status: 200,
            headers: headers(&[
                ("Content-Type", "text/plain; charset=utf-8"),
                ("Content-Encoding", "gzip"),
            ]),
            body: encoded,
            media_type: None,
        })
        .unwrap();
    let result = app
        .compare_evidence(
            request(
                exchange(&id),
                ComparisonSubject::Asset {
                    reference: asset.asset_ref(),
                },
            ),
            [None, None],
        )
        .await
        .unwrap();
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Exact body representation"
                && r.state == ComparisonState::NotCompared)
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Decoded bytes" && r.state == ComparisonState::Equal)
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.note.contains("original encoded entity bytes"))
    );
}

#[tokio::test]
async fn chunked_saz_measures_complete_entity_instead_of_comparing_transfer_framing() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chunked.saz");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, text) in [
        (
            "raw/1_c.txt",
            "GET https://chunked.test/ HTTP/1.1\r\nHost: chunked.test\r\n\r\n",
        ),
        (
            "raw/1_s.txt",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/plain\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
    ] {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let app = app(root.path());
    import(&app, path, "chunked").await;
    let id = app
        .query_sessions(crate::SessionQueryInput::default())
        .unwrap()
        .sessions[0]
        .id
        .clone();
    let plain = app
        .create_response_asset(crate::AuthoredResponseAsset {
            id: "plain".into(),
            revision: 1,
            status: 200,
            headers: headers(&[("Content-Type", "text/plain")]),
            body: b"hello".to_vec(),
            media_type: None,
        })
        .unwrap();
    let result = app
        .compare_evidence(
            request(
                exchange(&id),
                ComparisonSubject::Asset {
                    reference: plain.asset_ref(),
                },
            ),
            [None, None],
        )
        .await
        .unwrap();
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Decoded bytes" && r.state == ComparisonState::Equal)
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Exact body representation" && r.state == ComparisonState::Equal)
    );
}

#[tokio::test]
async fn stream_metadata_differences_keep_message_content_explicitly_uncompared() {
    use transmog_core::stream::{StreamProtocol, StreamState, StreamStatus};
    let summary = crate::StreamSummary {
        state: StreamState::now(StreamProtocol::WebSocket, StreamStatus::Open, None),
        imported: false,
        revision: 1,
        observed: 5,
        evicted: 0,
        sequence_loss: 0,
        retained: 5,
        client_bytes: Some(10),
        server_bytes: Some(20),
        opened_at: None,
        last_activity: None,
        lifetime: crate::StreamLifetime {
            duration_ms: None,
            established_at: None,
            ended_at: None,
            observed_until: None,
            live: false,
            boundary: "never-established",
        },
        http_duration_ms: None,
        response_headers_ms: None,
    };
    let mut left = evidence(headers(&[]));
    let mut right = evidence(headers(&[]));
    left.stream_indicated = true;
    right.stream_indicated = true;
    left.stream = Some(summary.clone());
    let mut other = summary;
    other.observed = 6;
    other.client_bytes = None;
    right.stream = Some(other);
    let registry = ComparisonRegistry::default();
    let op = registry.begin("streams").unwrap();
    let (rows, _, _) = algorithm::compare(&left, &right, &ComparisonOptions::default(), &op)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.field == "Observed stream records" && r.state == ComparisonState::Different)
    );
    assert!(rows.iter().any(|r|r.field=="Stream messages/events"&&r.state==ComparisonState::NotCompared));
    assert!(
        rows.iter()
            .any(|r| r.field == "Decoded client message bytes"
                && r.state == ComparisonState::Unavailable)
    );
}

#[tokio::test]
async fn expired_asset_revision_keeps_other_evidence_inspectable() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let absent = ComparisonSubject::Asset {
        reference: "removed@1".into(),
    };
    let draft = ComparisonSubject::Draft {
        label: "Retained proposal".into(),
        status: "404".into(),
        headers: HeaderBlock::default(),
        decoded_body: Some(b"proposal".to_vec()),
        base: None,
        body_path: None,
    };
    let result = app
        .compare_evidence(request(absent, draft), [None, None])
        .await
        .unwrap();
    assert!(!result.complete);
    assert!(
        result
            .rows
            .iter()
            .any(|r| r.field == "Boundary" && r.state == ComparisonState::Unavailable)
    );
    assert!(
        app.comparison_bytes(&result.id, "Response", 0)
            .unwrap()
            .right
            .contains("proposal")
    );
}
