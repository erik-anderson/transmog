use super::*;

async fn listen(h: &Harness, grant: &Grant, operation: &str, port: u16) -> Result<Value, Error> {
    h.call(
        grant,
        "transmog_session_listen",
        json!({"operationId":operation,"port":port,"decryptHttps":false}),
    )
    .await
}

async fn request_through(proxy: &str, origin: std::net::SocketAddr, path: &str) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut client = tokio::net::TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(
            format!(
                "GET http://{origin}/{path} HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&response)
    );
}

async fn origin_for_requests(count: usize) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = origin.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..count {
            let (mut socket, _) = origin.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
                .unwrap();
        }
    });
    (address, task)
}

#[tokio::test]
async fn full_start_history_does_not_block_another_key() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let other = h.grant(&[Permission::CollectPrivate]);
    let opened = listen(&h, &owner, "start", 0).await.unwrap();
    let address = opened["listener"]["endpoint"].as_str().unwrap();
    let port = address.parse::<std::net::SocketAddr>().unwrap().port();
    let mut reached_limit = false;
    for attempt in 0..512 {
        let failure = listen(&h, &other, &format!("occupied-{attempt}"), port)
            .await
            .unwrap_err();
        if failure.code == "limit" {
            assert!(failure.message.contains("receipt limit"));
            reached_limit = true;
            break;
        }
    }
    assert!(reached_limit);
    let second = listen(&h, &owner, "second", 0).await;
    let input = json!({"sessionId":opened["sessionId"],"operationId":"stop"});
    let stopped = h
        .call(&owner, "transmog_session_listener_stop", input.clone())
        .await
        .unwrap();
    assert_eq!(stopped["listener"]["lifecycle"], "stopped");
    assert_eq!(
        h.call(&owner, "transmog_session_listener_stop", input)
            .await
            .unwrap()["sessionId"],
        opened["sessionId"]
    );
    let _released = tokio::net::TcpListener::bind(address).await.unwrap();
    assert_eq!(second.unwrap()["listener"]["lifecycle"], "running");
    h.adapter.end_all().await.unwrap();
}

#[tokio::test]
async fn full_stop_history_cannot_evict_another_keys_receipt() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let other = h.grant(&[Permission::CollectPrivate]);
    let own_session = listen(&h, &owner, "start", 0).await.unwrap();
    let other_session = listen(&h, &other, "start", 0).await.unwrap();
    let stopped = h
        .call(
            &owner,
            "transmog_session_listener_stop",
            json!({"sessionId":own_session["sessionId"],"operationId":"stop"}),
        )
        .await
        .unwrap();
    for attempt in 0..256 {
        h.call(
            &other,
            "transmog_session_listener_stop",
            json!({"sessionId":other_session["sessionId"],"operationId":format!("stop-{attempt}")}),
        )
        .await
        .unwrap();
    }
    let receipt = h
        .call(
            &owner,
            "transmog_operation_status",
            json!({"operationId":"stop"}),
        )
        .await
        .unwrap();
    assert_eq!(receipt["result"], stopped);
    assert_eq!(
        h.call(
            &other,
            "transmog_operation_status",
            json!({"operationId":"stop-0"})
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    h.adapter.end_all().await.unwrap();
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Exercise the same three listener lifecycles through real traffic, recording, and cleanup."
)]
async fn private_listeners_isolate_traffic_and_recording_from_each_other_and_main() {
    let h = Harness::new();
    let owner = h
        .access
        .create(GrantInput {
            name: "private test".into(),
            permissions: [
                Permission::CollectPrivate,
                Permission::Inspect,
                Permission::ReadBodies,
                Permission::ReadCredentials,
            ]
            .into(),
            scope: Scope::All,
            omit_sensitive_values: false,
            expires_at: None,
        })
        .unwrap()
        .0;
    let other = h.grant(&[Permission::CollectPrivate, Permission::Inspect]);
    let shared_only = h.grant(&[Permission::Collect]);
    assert_eq!(
        listen(&h, &shared_only, "unauthorized", 0)
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
    let (one, retry) = tokio::join!(listen(&h, &owner, "one", 0), listen(&h, &owner, "one", 0));
    let one = one.unwrap();
    assert_eq!(one["sessionId"], retry.unwrap()["sessionId"]);
    let two = listen(&h, &owner, "two", 0).await.unwrap();
    let first = one["sessionId"].as_str().unwrap();
    let second = two["sessionId"].as_str().unwrap();
    let first_endpoint = one["listener"]["endpoint"].as_str().unwrap();
    let second_endpoint = two["listener"]["endpoint"].as_str().unwrap();
    assert_ne!(first_endpoint, second_endpoint);
    assert!(first_endpoint.starts_with("127.0.0.1:"));
    assert_eq!(one["listener"]["certificate"], Value::Null);
    let first_port = first_endpoint
        .parse::<std::net::SocketAddr>()
        .unwrap()
        .port();
    assert!(
        listen(&h, &owner, "collision", first_port)
            .await
            .unwrap_err()
            .message
            .contains("in use")
    );
    assert_eq!(
        listen(&h, &owner, "one", first_port)
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    assert!(
        h.call(&other, "transmog_traffic_list", json!({"sessionId":first}))
            .await
            .is_err()
    );
    assert_eq!(
        h.call(
            &owner,
            "transmog_session_live_subscribe",
            json!({"sessionId":first})
        )
        .await
        .unwrap_err()
        .code,
        "unsupported_session_operation"
    );
    assert!(
        h.call(&owner, "transmog_recording_start", json!({}))
            .await
            .is_err()
    );

    let recording = h
        .call(
            &owner,
            "transmog_recording_start",
            json!({"sessionId":first}),
        )
        .await
        .unwrap();
    let main = h
        .adapter
        .application
        .start_https_tunnels(transmog_app::ProxyStartRequest::default(), None)
        .await
        .unwrap();
    let (address, origin_task) = origin_for_requests(3).await;
    request_through(first_endpoint, address, "first").await;
    request_through(second_endpoint, address, "second").await;
    request_through(main.listener.as_deref().unwrap(), address, "main").await;
    origin_task.await.unwrap();
    h.adapter.fanout_live(None);
    for (id, path) in [(first, "/first"), (second, "/second"), ("main", "/main")] {
        let adapter = h.adapter.session_target(&owner, id).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let rows = adapter
                    .application
                    .query_sessions(SessionQueryInput::default())
                    .unwrap()
                    .sessions;
                if !rows.is_empty() {
                    assert_eq!(rows.len(), 1);
                    assert_eq!(rows[0].path, path);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    for _ in 0..2 {
        let stopped = h
            .call(
                &owner,
                "transmog_session_listener_stop",
                json!({"sessionId":first,"operationId":"stop-one"}),
            )
            .await
            .unwrap();
        assert_eq!(stopped["listener"]["lifecycle"], "stopped");
        assert_eq!(stopped["listener"]["recording"], false);
        assert_eq!(stopped["listener"]["recordingState"], "sealed");
    }
    let _port = tokio::net::TcpListener::bind(first_endpoint).await.unwrap();
    assert!(
        tokio::net::TcpStream::connect(second_endpoint)
            .await
            .is_ok()
    );
    assert!(
        h.call(&owner, "transmog_traffic_list", json!({"sessionId":first}))
            .await
            .is_ok()
    );
    h.call(&owner, "transmog_session_close", json!({"sessionId":first}))
        .await
        .unwrap();
    let retry = listen(&h, &owner, "one", 0).await.unwrap();
    assert_eq!(retry["sessionId"], first);
    assert_eq!(retry["agentAccessEnded"], true);
    assert_eq!(retry["listener"]["endpoint"], Value::Null);
    assert_eq!(retry["listener"]["recordingState"], "sealed");
    assert_eq!(
        retry["listener"]["recordingArtifact"],
        recording["artifact"]
    );
    let start_status = h
        .call(
            &owner,
            "transmog_operation_status",
            json!({"operationId":"one"}),
        )
        .await
        .unwrap();
    assert_eq!(start_status["result"], retry);
    let receipt = h
        .call(
            &owner,
            "transmog_operation_status",
            json!({"operationId":"stop-one"}),
        )
        .await
        .unwrap();
    assert_eq!(receipt["state"], "completed");
    assert_eq!(receipt["result"]["sessionId"], first);
    assert!(
        h.call(
            &other,
            "transmog_operation_status",
            json!({"operationId":"stop-one"})
        )
        .await
        .is_err()
    );
    assert!(
        h.adapter
            .root
            .join("agent-exports")
            .join(recording["artifact"].as_str().unwrap())
            .is_file()
    );
    h.adapter.end_all().await.unwrap();
    assert!(
        tokio::net::TcpStream::connect(second_endpoint)
            .await
            .is_err()
    );
    assert_eq!(
        h.adapter.application.status().lifecycle,
        transmog_app::AppLifecycle::Running
    );
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn private_listener_expiry_stops_even_with_a_review_open() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let opened = listen(&h, &owner, "expiring", 0).await.unwrap();
    let id = opened["sessionId"].as_str().unwrap();
    let endpoint = opened["listener"]["endpoint"].as_str().unwrap();
    let review = h.adapter.review_session(id).unwrap();
    review.set_dirty(true);
    h.adapter.expire_session_for_test(id);
    let listed = h
        .call(&owner, "transmog_session_list", json!({}))
        .await
        .unwrap();
    let receipt = listen(&h, &owner, "expiring", 0).await.unwrap();
    assert_eq!(listed["sessions"], json!([]));
    assert_eq!(receipt["agentAccessEnded"], true);
    assert_eq!(receipt["listener"]["endpoint"], Value::Null);
    assert_eq!(receipt["listener"]["proxyUrl"], Value::Null);
    h.adapter.maintain_sessions().await.unwrap();
    assert!(review.access_ended());
    assert!(review.is_dirty());
    assert_eq!(
        review.application().status().lifecycle,
        transmog_app::AppLifecycle::Stopped
    );
    let _port = tokio::net::TcpListener::bind(endpoint).await.unwrap();
}

#[tokio::test]
async fn closing_review_does_not_renew_an_expired_private_listener() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let opened = listen(&h, &owner, "review-close", 0).await.unwrap();
    let id = opened["sessionId"].as_str().unwrap();
    let endpoint = opened["listener"]["endpoint"].as_str().unwrap();
    let review = h.adapter.review_session(id).unwrap();
    h.adapter.expire_session_for_test(id);
    drop(review);

    assert_eq!(
        h.call(&owner, "transmog_session_renew", json!({"sessionId":id}))
            .await
            .unwrap_err()
            .code,
        "session_unavailable"
    );
    h.adapter.maintain_sessions().await.unwrap();
    let _port = tokio::net::TcpListener::bind(endpoint).await.unwrap();
}

#[tokio::test]
async fn reopening_mcp_admits_new_listeners_without_reviving_old_sessions() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let opened = listen(&h, &owner, "before-disable", 0).await.unwrap();
    let endpoint = opened["listener"]["endpoint"].as_str().unwrap();
    h.access.set_enabled(false).unwrap();
    h.adapter.end_all().await.unwrap();
    let _released = tokio::net::TcpListener::bind(endpoint).await.unwrap();
    h.access.set_enabled(true).unwrap();
    assert_eq!(
        listen(&h, &owner, "still-closed", 0)
            .await
            .unwrap_err()
            .code,
        "unavailable"
    );
    h.adapter.open_session_registry();
    let retried = listen(&h, &owner, "before-disable", 0).await.unwrap();
    assert_eq!(retried["sessionId"], opened["sessionId"]);
    assert_eq!(retried["agentAccessEnded"], true);
    let restarted = listen(&h, &owner, "after-enable", 0).await.unwrap();
    assert_ne!(restarted["sessionId"], opened["sessionId"]);
    assert_eq!(restarted["listener"]["lifecycle"], "running");
    h.adapter.end_all().await.unwrap();
}

#[tokio::test]
async fn private_listener_close_keeps_running_after_the_caller_disconnects() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let opened = listen(&h, &owner, "disconnect", 0).await.unwrap();
    let id = opened["sessionId"].as_str().unwrap();
    let endpoint = opened["listener"]["endpoint"].as_str().unwrap();
    let review = h.adapter.review_session(id).unwrap();
    let session = h.adapter.session_target(&owner, id).unwrap();
    let locked = session.operations.lock().await;
    let root = h.adapter.clone();
    let input = json!({"instanceId":root.instance_id(),"sessionId":id});
    let caller =
        tokio::spawn(async move { root.handle(owner, "transmog_session_close", input).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !review.access_ended() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(locked);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(_listener) = tokio::net::TcpListener::bind(endpoint).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn closing_a_private_listener_releases_its_port_and_preserves_dirty_review() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let opened = listen(&h, &owner, "dirty-review", 0).await.unwrap();
    let id = opened["sessionId"].as_str().unwrap();
    let endpoint = opened["listener"]["endpoint"].as_str().unwrap();
    let review = h.adapter.review_session(id).unwrap();
    review.set_dirty(true);
    assert_eq!(
        h.call(&owner, "transmog_session_close", json!({"sessionId":id}))
            .await
            .unwrap(),
        json!({"closed":true})
    );
    assert!(review.access_ended());
    assert!(review.is_dirty());
    let _port = tokio::net::TcpListener::bind(endpoint).await.unwrap();
}

#[test]
fn disconnected_recording_start_keeps_ownership() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            let h = Harness::new();
            let owner = h
                .access
                .create(GrantInput {
                    name: "Recording owner".into(),
                    permissions: [
                        Permission::CollectPrivate,
                        Permission::ReadBodies,
                        Permission::ReadCredentials,
                    ]
                    .into(),
                    scope: Scope::All,
                    omit_sensitive_values: false,
                    expires_at: None,
                })
                .unwrap()
                .0;
            let opened = listen(&h, &owner, "disconnect-during-recording-start", 0)
                .await
                .unwrap();
            let id = opened["sessionId"].as_str().unwrap();
            let session = h.adapter.session_target(&owner, id).unwrap();
            // Keep the writer command pending until the admitted request is dropped.
            let (release, permit) = tokio::sync::oneshot::channel();
            let blocking_task = tokio::task::spawn_blocking(move || permit.blocking_recv());
            tokio::select! {
                biased;
                result = h.call(&owner, "transmog_recording_start", json!({"sessionId":id})) => {
                    panic!("The writer completed while blocked: {result:?}");
                }
                () = std::future::ready(()) => {}
            }
            release.send(()).unwrap();
            blocking_task.await.unwrap().unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while !matches!(
                    session.application.capture_status(),
                    transmog_app::CaptureReadModel::Active { .. }
                ) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();

            let stopped = h
                .call(&owner, "transmog_recording_stop", json!({"sessionId":id}))
                .await
                .unwrap();
            assert_eq!(stopped["sealed"], true);
            h.adapter.end_all().await.unwrap();
        });
}

#[tokio::test]
async fn recording_waiting_for_admission_rechecks_temporary_access() {
    let h = Harness::new();
    let owner = h
        .access
        .create(GrantInput {
            name: "Recording owner".into(),
            permissions: [Permission::CollectPrivate].into(),
            scope: Scope::All,
            omit_sensitive_values: true,
            expires_at: None,
        })
        .unwrap()
        .0;
    let opened = listen(&h, &owner, "recording-approval", 0).await.unwrap();
    let id = opened["sessionId"].as_str().unwrap();
    let approval = h
        .access
        .request_sensitive_access(&owner, "Capture a local application issue.")
        .unwrap();
    let approval_id = approval["request"]["id"].as_str().unwrap();
    h.access
        .decide_sensitive_access(approval_id, "minute")
        .unwrap();
    let approved = h.access.session_authority(&owner.id).unwrap();
    let session = h.adapter.session_target(&owner, id).unwrap();
    let locked = session.operations.lock().await;
    let mut recording = std::pin::pin!(h.call(
        &approved,
        "transmog_recording_start",
        json!({"sessionId":id})
    ));
    tokio::select! {
        biased;
        result = &mut recording => panic!("The request bypassed the operation lock: {result:?}"),
        () = std::future::ready(()) => {}
    }
    h.access.revoke_sensitive_access(approval_id).unwrap();
    drop(locked);

    assert_eq!(
        recording.await.unwrap_err().code,
        "sensitive_access_required"
    );
    assert!(matches!(
        session.application.capture_status(),
        transmog_app::CaptureReadModel::Idle
    ));
    assert!(!h.adapter.root.join("agent-exports").exists());
    h.adapter.end_all().await.unwrap();
}

#[tokio::test]
async fn closing_after_a_recording_failure_releases_the_session_and_notifies_review() {
    use transmog_app::CaptureReadModel;

    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let opened = listen(&h, &owner, "failed-capture", 0).await.unwrap();
    let id = opened["sessionId"].as_str().unwrap();
    let endpoint = opened["listener"]["endpoint"].as_str().unwrap();
    let review = h.adapter.review_session(id).unwrap();
    let application = review.application();
    let path = h.adapter.root.join("failed-close.tmcap");
    application
        .start_capture(CaptureStartRequest {
            path: path.clone(),
            password: None,
            include_network_context: false,
            max_file_bytes: 64 * 1024,
            retain_body_samples: true,
            redact_sensitive_headers: true,
        })
        .await
        .unwrap();
    let (address, origin_task) = origin_for_requests(3).await;
    for number in 0..3 {
        request_through(endpoint, address, &format!("request-{number}")).await;
    }
    origin_task.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !matches!(
            application.capture_status(),
            CaptureReadModel::Failed { .. }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let changes = h.adapter.subscribe_changes();
    let error = h
        .call(&owner, "transmog_session_close", json!({"sessionId":id}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "operation_failed");
    assert!(error.message.contains("capture"));
    assert!(review.access_ended());
    let _port = tokio::net::TcpListener::bind(endpoint).await.unwrap();
    assert_eq!(h.adapter.session_summaries(None), Vec::<Value>::new());
    assert!(changes.has_changed().unwrap());
    let receipt = listen(&h, &owner, "failed-capture", 0).await.unwrap();
    assert_eq!(receipt["sessionId"], id);
    assert_eq!(receipt["agentAccessEnded"], true);
    assert_eq!(receipt["listener"]["endpoint"], Value::Null);
    assert_eq!(receipt["listener"]["recordingState"], "failed");
    assert_eq!(
        receipt["listener"]["recordingArtifact"],
        "failed-close.tmcap"
    );
    let recovered = application
        .import_capture(transmog_app::ImportRequest {
            path,
            password: None,
            max_file_bytes: u64::MAX,
        })
        .await
        .unwrap();
    assert!(recovered.records > 0);
    assert!(!recovered.sealed);
}

#[tokio::test]
async fn decrypted_private_listener_owns_public_certificate_and_revocation_releases_port() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::CollectPrivate]);
    let opened = h
        .call(
            &owner,
            "transmog_session_listen",
            json!({"operationId":"decrypt","decryptHttps":true}),
        )
        .await
        .unwrap();
    assert_eq!(opened["listener"]["decryptHttps"], true);
    let certificate = &opened["listener"]["certificate"];
    let path = PathBuf::from(certificate["certificatePath"].as_str().unwrap());
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("-----BEGIN CERTIFICATE-----")
    );
    assert_eq!(certificate["sha256"].as_str().unwrap().len(), 64);
    let endpoint = opened["listener"]["endpoint"].as_str().unwrap();
    h.access.revoke(Some((&owner.id, owner.revision))).unwrap();
    h.adapter.reconcile().await.unwrap();
    assert_eq!(h.adapter.session_summaries(None), Vec::<Value>::new());
    assert!(!path.exists());
    let _port = tokio::net::TcpListener::bind(endpoint).await.unwrap();
}

async fn session(h: &Harness, grant: &Grant) -> String {
    h.call(
        grant,
        "transmog_session_create",
        json!({"name":"Investigation"}),
    )
    .await
    .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .into()
}
async fn settled(h: &Harness, grant: &Grant, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let result = h
                .call(
                    grant,
                    "transmog_operation_status",
                    json!({"operationId":id}),
                )
                .await
                .unwrap();
            if result["state"] != "pending" {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn har(h: &Harness) -> PathBuf {
    let path = h.adapter.root.as_path().join("capture.har");
    std::fs::write(&path,serde_json::to_vec(&json!({"log":{"version":"1.2","creator":{"name":"Fixture","version":"1"},"entries":[{"startedDateTime":"2026-01-01T00:00:00Z","time":1,"request":{"method":"GET","url":"http://example.test/context","httpVersion":"HTTP/1.1","headers":[{"name":"Authorization","value":"hidden-secret"}],"cookies":[],"queryString":[],"headersSize":-1,"bodySize":0},"response":{"status":200,"statusText":"OK","httpVersion":"HTTP/1.1","headers":[],"cookies":[],"headersSize":-1,"bodySize":7,"content":{"size":7,"mimeType":"text/plain","text":"payload"},"redirectURL":""},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}]}})).unwrap()).unwrap();
    path
}

async fn cancel_password_load(h: &Harness, owner: &Grant, session: &str, path: &std::path::Path) {
    h.call(
        owner,
        "transmog_capture_load",
        json!({"sessionId":session,"operationId":"cancel-password","path":path}),
    )
    .await
    .unwrap();
    assert_eq!(
        settled(h, owner, "cancel-password").await["state"],
        "waiting-password"
    );
    let cancelled = h
        .call(
            owner,
            "transmog_operation_cancel",
            json!({"operationId":"cancel-password"}),
        )
        .await
        .unwrap();
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["cancelRequested"], true);
    assert!(
        h.adapter
            .supply_capture_password(session, "cancel-password", "capture passphrase".into())
            .is_err()
    );
}

#[tokio::test]
async fn private_capture_load_unload_is_idempotent_key_owned_and_releases_context() {
    let h = Harness::new();
    let owner = h.grant(&[
        Permission::Inspect,
        Permission::LoadCaptures,
        Permission::ReadBodies,
        Permission::ReadCredentials,
    ]);
    let other = h.grant(&[Permission::Inspect, Permission::LoadCaptures]);
    let id = session(&h, &owner).await;
    assert!(
        h.call(&other, "transmog_traffic_list", json!({"sessionId":id}))
            .await
            .is_err()
    );
    assert_eq!(
        h.call(&other, "transmog_session_list", json!({}))
            .await
            .unwrap()["sessions"],
        json!([])
    );
    let load = json!({"sessionId":id,"operationId":"load","path":har(&h)});
    let review = h.adapter.review_session(&id).unwrap();
    review.set_dirty(true);
    h.call(&owner, "transmog_capture_load", load.clone())
        .await
        .unwrap();
    let completed = settled(&h, &owner, "load").await;
    assert_eq!(completed["state"], "completed");
    assert!(review.is_dirty());
    review.set_dirty(false);
    assert_eq!(
        h.call(&owner, "transmog_capture_load", load).await.unwrap()["result"],
        completed["result"]
    );
    assert_eq!(
        h.call(
            &owner,
            "transmog_capture_load",
            json!({"sessionId":id,"operationId":"load","path":"other.har"})
        )
        .await
        .unwrap_err()
        .code,
        "conflict"
    );
    let page = h
        .call(&owner, "transmog_traffic_list", json!({"sessionId":id}))
        .await
        .unwrap();
    assert_eq!(page["totalMatched"], 1);
    assert_eq!(
        h.call(&owner, "transmog_traffic_list", json!({"sessionId":"main"}))
            .await
            .unwrap()["totalMatched"],
        0
    );
    let entry = &page["sessions"][0]["id"];
    let body = h
        .call(
            &owner,
            "transmog_body_read",
            json!({"sessionId":id,"id":entry,"boundary":"client-response","decodeContent":true}),
        )
        .await
        .unwrap();
    let capture = &completed["result"]["captureId"];
    h.call(
        &owner,
        "transmog_capture_unload",
        json!({"sessionId":id,"captureId":capture,"operationId":"unload"}),
    )
    .await
    .unwrap();
    assert_eq!(settled(&h, &owner, "unload").await["state"], "completed");
    assert!(
        h.call(
            &owner,
            "transmog_context_read",
            json!({"sessionId":id,"contextId":body["contextId"]})
        )
        .await
        .is_err()
    );
    assert_eq!(
        h.call(&owner, "transmog_capture_list", json!({"sessionId":id}))
            .await
            .unwrap()["captures"],
        json!([])
    );
    assert!(h.adapter.root.as_path().join("capture.har").exists());
    h.call(&owner, "transmog_session_close", json!({"sessionId":id}))
        .await
        .unwrap();
    assert!(
        h.call(&owner, "transmog_traffic_list", json!({"sessionId":id}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn capture_permission_is_separate_and_review_preserves_dirty_work_after_revocation() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::Inspect]);
    let id = session(&h, &owner).await;
    assert_eq!(
        h.call(
            &owner,
            "transmog_capture_load",
            json!({"sessionId":id,"operationId":"load","path":har(&h)})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    let review = h.adapter.review_session(&id).unwrap();
    review.set_dirty(true);
    assert_eq!(
        h.call(&owner, "transmog_session_close", json!({"sessionId":id}))
            .await
            .unwrap_err()
            .code,
        "needs_human_action"
    );
    h.access.revoke(Some((&owner.id, owner.revision))).unwrap();
    h.adapter.maintain_sessions().await.unwrap();
    assert!(review.access_ended());
    assert!(review.is_dirty());
    assert!(h.adapter.session_record_current(&owner, &id).is_err());
    review.set_dirty(false);
    h.adapter.maintain_sessions().await.unwrap();
    assert_eq!(
        h.adapter.session_summaries(None),
        [] as [serde_json::Value; 0]
    );
}

#[tokio::test]
async fn human_private_rules_are_editable_by_the_owner_and_publish_disabled_with_revision_checks() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::PrepareChanges, Permission::EditSharedRules]);
    let id = session(&h, &owner).await;
    let adapter = h.adapter.session_target(&owner, &id).unwrap();
    let human = adapter
        .save_rule(rule("human-private", false), None)
        .unwrap();
    let changed=h.call(&owner,"transmog_rule_update",json!({"sessionId":id,"id":human["id"],"expectedRevision":human["revision"],"rule":authoring::definition(&rule("ignored",false)).unwrap()})).await.unwrap();
    assert_eq!(
        h.call(
            &owner,
            "transmog_rule_publish",
            json!({"sessionId":id,"id":human["id"],"expectedRevision":human["revision"]})
        )
        .await
        .unwrap_err()
        .code,
        "conflict"
    );
    let published = h
        .call(
            &owner,
            "transmog_rule_publish",
            json!({"sessionId":id,"id":changed["id"],"expectedRevision":changed["revision"]}),
        )
        .await
        .unwrap();
    assert_eq!(published["enabled"], false);
    h.call(&owner, "transmog_session_close", json!({"sessionId":id}))
        .await
        .unwrap();
    assert_eq!(h.adapter.application.automation_status().rules.len(), 1);
}

#[tokio::test]
async fn idle_expiry_releases_sessions_but_reviewing_keeps_them_alive() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::Inspect]);
    let id = session(&h, &owner).await;
    h.adapter.expire_session_for_test(&id);
    let review = h.adapter.review_session(&id).unwrap();
    h.adapter.expire_session_for_test(&id);
    h.adapter.maintain_sessions().await.unwrap();
    assert_ne!(
        h.adapter.session_summaries(None),
        [] as [serde_json::Value; 0]
    );
    drop(review);
    h.adapter.expire_session_for_test(&id);
    h.adapter.maintain_sessions().await.unwrap();
    assert_eq!(
        h.adapter.session_summaries(None),
        [] as [serde_json::Value; 0]
    );
}

#[tokio::test]
async fn encrypted_load_waits_for_trusted_password_without_changing_redaction() {
    let h = Harness::new();
    let owner = h
        .access
        .create(GrantInput {
            name: "Protected reader".into(),
            permissions: [
                Permission::Inspect,
                Permission::LoadCaptures,
                Permission::ReadBodies,
                Permission::ReadCredentials,
            ]
            .into_iter()
            .collect(),
            scope: Scope::All,
            expires_at: None,
            omit_sensitive_values: true,
        })
        .unwrap()
        .0;
    let seed = h
        .adapter
        .application
        .create_inspection_session(h.adapter.root.as_path().join("seed"))
        .unwrap();
    seed.import_trace(
        transmog_app::TraceImportRequest {
            path: har(&h),
            operation_id: "seed".into(),
            password: None,
            max_file_bytes: 1024 * 1024,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    let path = h.adapter.root.as_path().join("encrypted.tmcap");
    seed.save_traffic_trace(
        path.clone(),
        transmog_app::TraceSaveOptions {
            password: Some(serde_json::from_value(json!("capture passphrase")).unwrap()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    seed.discard_traffic().unwrap();
    let id = session(&h, &owner).await;
    h.call(
        &owner,
        "transmog_capture_load",
        json!({"sessionId":id,"path":path,"operationId":"encrypted"}),
    )
    .await
    .unwrap();
    let waiting = settled(&h, &owner, "encrypted").await;
    assert_eq!(waiting["state"], "waiting-password");
    cancel_password_load(&h, &owner, &id, &path).await;
    assert_eq!(
        h.call(&owner, "transmog_traffic_list", json!({"sessionId":id}))
            .await
            .unwrap()["totalMatched"],
        0
    );
    h.adapter
        .supply_capture_password(&id, "encrypted", "wrong phrase".into())
        .unwrap();
    assert_eq!(
        settled(&h, &owner, "encrypted").await["state"],
        "waiting-password"
    );
    h.adapter
        .supply_capture_password(&id, "encrypted", "capture passphrase".into())
        .unwrap();
    let result = settled(&h, &owner, "encrypted").await;
    assert_eq!(result["state"], "completed");
    assert!(!result.to_string().contains("capture passphrase"));
    assert!(
        !h.access
            .session_authority(&owner.id)
            .unwrap()
            .allows(Permission::ReadBodies)
    );
    let listed = h
        .call(&owner, "transmog_traffic_list", json!({"sessionId":id}))
        .await
        .unwrap();
    assert_eq!(
        h.call(
            &owner,
            "transmog_body_read",
            json!({"sessionId":id,"id":listed["sessions"][0]["id"],"boundary":"client-response"})
        )
        .await
        .unwrap_err()
        .code,
        "sensitive_access_required"
    );
}

#[tokio::test]
async fn closing_releases_storage_after_the_last_review_handle() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::Inspect]);
    let id = session(&h, &owner).await;
    let adapter = h.adapter.session_target(&owner, &id).unwrap();
    let directory = adapter.root.clone();
    drop(adapter);
    let review = h.adapter.review_session(&id).unwrap();
    h.call(&owner, "transmog_session_close", json!({"sessionId":id}))
        .await
        .unwrap();
    assert!(directory.exists());
    assert!(review.access_ended());
    drop(review);
    assert!(!directory.exists());
}

#[tokio::test]
async fn capture_context_survives_view_clear_and_is_invalid_after_explicit_unload() {
    let h = Harness::new();
    let owner = h
        .access
        .create(GrantInput {
            name: "Capture context".into(),
            permissions: [
                Permission::Inspect,
                Permission::LoadCaptures,
                Permission::ReadBodies,
                Permission::ReadCredentials,
            ]
            .into_iter()
            .collect(),
            scope: Scope::All,
            omit_sensitive_values: false,
            expires_at: None,
        })
        .unwrap()
        .0;
    let id = session(&h, &owner).await;
    h.call(
        &owner,
        "transmog_capture_load",
        json!({"sessionId":id,"operationId":"context","path":har(&h)}),
    )
    .await
    .unwrap();
    let loaded = settled(&h, &owner, "context").await;
    assert_eq!(loaded["state"], "completed");
    let target = h.adapter.session_target(&owner, &id).unwrap();
    target.application.discard_traffic().unwrap();
    let captures = h
        .call(
            &owner,
            "transmog_capture_list",
            json!({"sessionId":id,"includeContext":true}),
        )
        .await
        .unwrap();
    let context = &captures["captures"][0]["context"]["contextId"];
    h.call(
        &owner,
        "transmog_context_read",
        json!({"sessionId":id,"contextId":context,"maxBytes":31}),
    )
    .await
    .unwrap();
    target
        .application
        .unload_capture(loaded["result"]["captureId"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        h.call(
            &owner,
            "transmog_context_read",
            json!({"sessionId":id,"contextId":context})
        )
        .await
        .unwrap_err()
        .code,
        "source_unloaded"
    );
}

#[tokio::test]
async fn files_added_in_the_review_backend_are_listed_and_unloadable() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::Inspect, Permission::LoadCaptures]);
    let id = session(&h, &owner).await;
    let review = h.adapter.review_session(&id).unwrap();
    let imported = review
        .application()
        .import_trace(
            transmog_app::TraceImportRequest {
                path: har(&h),
                operation_id: "human-import".into(),
                password: None,
                max_file_bytes: 1024 * 1024,
            },
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    let captures = h
        .call(&owner, "transmog_capture_list", json!({"sessionId":id}))
        .await
        .unwrap();
    assert_eq!(captures["captures"][0]["captureId"], imported.trace.id);
    h.call(
        &owner,
        "transmog_capture_unload",
        json!({"sessionId":id,"captureId":imported.trace.id,"operationId":"unload-human-import"}),
    )
    .await
    .unwrap();
    assert_eq!(
        settled(&h, &owner, "unload-human-import").await["state"],
        "completed"
    );
    assert_eq!(review.application().loaded_capture_ids(), [] as [String; 0]);
}
