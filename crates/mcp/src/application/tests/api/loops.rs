use super::*;

async fn context_bytes(h: &Harness, grant: &Grant, reference: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = json!("0");
    loop {
        let page = h
            .call(
                grant,
                "transmog_context_read",
                json!({"contextId":reference["contextId"],"offset":offset,"maxBytes":17003}),
            )
            .await
            .unwrap();
        contract("transmog_context_read", &page);
        bytes.extend(if page["encoding"] == "utf-8" {
            page["data"].as_str().unwrap().as_bytes().to_vec()
        } else {
            STANDARD.decode(page["data"].as_str().unwrap()).unwrap()
        });
        if page["nextOffset"].is_null() {
            break;
        }
        offset = page["nextOffset"].clone();
    }
    bytes
}

#[tokio::test]
async fn complete_decoded_context_pages_preserve_unicode_and_recheck_authority() {
    let h = traffic(serde_json::to_vec(&json!({"data":"λ🦀".repeat(30000)})).unwrap());
    let owner = full(&h);
    let sent = send(&h, &owner, "large").await;
    let first=h.call(&owner,"transmog_body_read",json!({"id":sent["id"],"boundary":"client-response","decodeContent":true,"maxBytes":17})).await.unwrap();
    contract("transmog_body_read", &first);
    assert!(first["nextOffset"].is_string());
    assert_eq!(context_bytes(&h, &owner, &first).await, h.body);
    let outsider = full(&h);
    assert_eq!(
        h.call(
            &outsider,
            "transmog_context_read",
            json!({"contextId":first["contextId"]})
        )
        .await
        .unwrap_err()
        .code,
        "expired_context"
    );
    let mut revised = owner.clone();
    revised.revision += 1;
    assert_eq!(
        h.call(
            &revised,
            "transmog_context_read",
            json!({"contextId":first["contextId"]})
        )
        .await
        .unwrap_err()
        .code,
        "expired_context"
    );
    let mut protected = owner.clone();
    protected.permissions.remove(&Permission::ReadBodies);
    assert_eq!(
        h.call(
            &protected,
            "transmog_context_read",
            json!({"contextId":first["contextId"]})
        )
        .await
        .unwrap_err()
        .code,
        "sensitive_access_required"
    );
    let mut outside = owner.clone();
    outside.scope = Scope::Hosts {
        hosts: vec!["elsewhere.test".into()],
    };
    assert_eq!(
        h.call(
            &outside,
            "transmog_context_read",
            json!({"contextId":first["contextId"]})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    h.adapter.application.shutdown().await.unwrap();
}

async fn live_send(h: &TrafficHarness, owner: &Grant, operation: &str) -> (Value, String) {
    let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = origin.local_addr().unwrap();
    let body = h.body.clone();
    let producer = tokio::spawn(async move {
        let (mut socket, _) = origin.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nX-Long: {}\r\nConnection: close\r\n\r\n",body.len(),"v".repeat(6000)).as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        String::from_utf8(headers).unwrap()
    });
    let sent=h.call(owner,"transmog_request_send",json!({"operationId":operation,"mode":"live-rules","request":{"method":"GET","url":format!("http://{address}/live")}})).await.unwrap();
    contract("transmog_request_send", &sent);
    (sent, producer.await.unwrap())
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Verify one complete live-rule edit, replay and review lifecycle."
)]
async fn live_rules_replay_retains_all_boundaries_and_tests_complete_captured_samples() {
    let h =
        traffic(serde_json::to_vec(&json!({"data":"x".repeat(180_000),"label":"before"})).unwrap());
    let owner = h.grant(&[
        Permission::Inspect,
        Permission::Send,
        Permission::ReadBodies,
        Permission::ReadCredentials,
        Permission::PrepareChanges,
        Permission::Modify,
    ]);
    send(&h, &owner, "seed").await;
    assert_eq!(h.call(&owner,"transmog_request_send",json!({"operationId":"https-disabled","mode":"live-rules","request":{"method":"GET","url":"https://127.0.0.1/"}})).await.unwrap_err().code,"invalid_input");
    assert_eq!(
        h.call(
            &owner,
            "transmog_request_status",
            json!({"operationId":"https-disabled"})
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    let rule=h.call(&owner,"transmog_rule_prepare",json!({"rule":{"kind":"headers","name":"Live debug header","host":"127.0.0.1","phase":"request","operations":[{"operation":"set","name":"X-Debug","value":"verified"}]}})).await.unwrap();
    let control = h
        .call(&owner, "transmog_control_begin", json!({}))
        .await
        .unwrap();
    h.call(
        &owner,
        "transmog_rule_activate",
        json!({"id":rule["id"],"expectedRevision":rule["revision"],"controlId":control["id"]}),
    )
    .await
    .unwrap();
    let (first, headers) = live_send(&h, &owner, "live-a").await;
    assert!(headers.to_lowercase().contains("x-debug: verified"));
    live_send(&h, &owner, "live-b").await;
    let exchange = h
        .call(
            &owner,
            "transmog_exchange_get",
            json!({"id":first["id"],"include":["all"]}),
        )
        .await
        .unwrap();
    contract("transmog_exchange_get", &exchange);
    assert_eq!(exchange["requests"].as_array().unwrap().len(), 2);
    assert_eq!(exchange["responses"].as_array().unwrap().len(), 2);
    assert!(
        exchange["changes"]
            .to_string()
            .contains("Live debug header")
    );
    let complete: Value =
        serde_json::from_slice(&context_bytes(&h, &owner, &exchange["context"]).await).unwrap();
    assert!(complete.get("caller").is_some());
    assert!(!complete.to_string().contains("authRevisions"));
    let headers = h
        .call(
            &owner,
            "transmog_headers_read",
            json!({"id":first["id"],"boundary":"client-response"}),
        )
        .await
        .unwrap();
    contract("transmog_headers_read", &headers);
    assert_eq!(
        headers["headers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case("X-Long")))
            .unwrap()["value"]
            .as_str()
            .unwrap()
            .len(),
        6000
    );
    let text=h.call(&owner,"transmog_rule_prepare",json!({"rule":{"kind":"response-text","name":"Replace captured label","host":"127.0.0.1","find":"before","replacement":"after","mediaType":"application/json"}})).await.unwrap();
    let tested = h
        .call(
            &owner,
            "transmog_rule_test",
            json!({"id":text["id"],"sampleExchangeId":first["id"]}),
        )
        .await
        .unwrap();
    contract("transmog_rule_test", &tested);
    assert_eq!(tested["inlineComplete"], false);
    let result: Value =
        serde_json::from_slice(&context_bytes(&h, &owner, &tested["context"]).await).unwrap();
    assert_eq!(result["beforeText"].as_str().unwrap().len(), h.body.len());
    assert!(
        result["afterText"]
            .as_str()
            .unwrap()
            .ends_with("\"after\"}")
    );
    let mut protected = owner.clone();
    protected.permissions.remove(&Permission::ReadBodies);
    assert_eq!(
        h.call(
            &protected,
            "transmog_context_read",
            json!({"contextId":tested["context"]["contextId"]})
        )
        .await
        .unwrap_err()
        .code,
        "sensitive_access_required"
    );
    let mut selected = owner.clone();
    selected.scope = Scope::Requests {
        instance_id: h.adapter.instance_id.clone(),
        ids: vec![first["id"].as_str().unwrap().into()],
    };
    assert!(
        h.call(
            &selected,
            "transmog_rule_test",
            json!({"id":text["id"],"sampleExchangeId":first["id"]})
        )
        .await
        .is_ok()
    );
    let body = h
        .call(
            &owner,
            "transmog_body_read",
            json!({"id":first["id"],"boundary":"upstream-response","offset":"170000"}),
        )
        .await
        .unwrap();
    assert_eq!(
        STANDARD.decode(body["data"].as_str().unwrap()).unwrap(),
        h.body[170_000..]
    );
    h.adapter.application.shutdown().await.unwrap();
}

fn replay_harness(executor: Arc<dyn transmog_session::ReplayExecutor>) -> Harness {
    let root = tempfile::tempdir().unwrap();
    let access = AccessManager::open(None).unwrap();
    access.set_enabled(true).unwrap();
    let application = Application::new(AppConfig {
        replay_executor: Some(executor),
        ..Default::default()
    })
    .unwrap();
    let adapter = AgentApplication::new(
        application,
        access.clone(),
        root.path().into(),
        Arc::new(NoProxy),
    )
    .unwrap();
    Harness {
        _root: root,
        adapter,
        access,
    }
}

#[tokio::test]
async fn send_preflight_does_not_poison_ids_and_old_evidence_and_status_remain_readable() {
    let executor = Arc::new(ResponseFixture(std::sync::atomic::AtomicUsize::new(0)));
    let h = replay_harness(executor.clone());
    let owner = h.grant(&[
        Permission::Inspect,
        Permission::Send,
        Permission::ReadBodies,
    ]);
    let mut request =
        json!({"method":"POST","url":"http://example.test/","body":"complete-request"});
    for _ in 0..2 {
        assert_eq!(
            h.call(
                &owner,
                "transmog_request_send",
                json!({"operationId":"one","request":request})
            )
            .await
            .unwrap_err()
            .code,
            "invalid_input"
        );
    }
    assert_eq!(executor.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    request["acknowledgeNonIdempotent"] = json!(true);
    let first = h
        .call(
            &owner,
            "transmog_request_send",
            json!({"operationId":"one","request":request}),
        )
        .await
        .unwrap();
    h.call(
        &owner,
        "transmog_request_send",
        json!({"operationId":"two","request":request}),
    )
    .await
    .unwrap();
    let status = h
        .call(
            &owner,
            "transmog_request_status",
            json!({"operationId":"one"}),
        )
        .await
        .unwrap();
    contract("transmog_request_status", &status);
    assert_eq!(status["result"], first);
    let body = h
        .call(
            &owner,
            "transmog_body_read",
            json!({"id":first["id"],"boundary":"client-request"}),
        )
        .await
        .unwrap();
    assert_eq!(
        STANDARD.decode(body["data"].as_str().unwrap()).unwrap(),
        b"complete-request"
    );
    let prepared = h
        .call(
            &owner,
            "transmog_request_prepare",
            json!({"id":first["id"],"includeBody":true}),
        )
        .await
        .unwrap();
    let mut draft: ComposerRequest = parse(prepared["request"].clone()).unwrap();
    draft.acknowledge_non_idempotent = true;
    h.adapter
        .application
        .validate_agent_request(&draft, false)
        .unwrap();
    let stranger = h.grant(&[Permission::Send]);
    assert_eq!(
        h.call(
            &stranger,
            "transmog_request_status",
            json!({"operationId":"one"})
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    h.adapter.application.shutdown().await.unwrap();
}

struct PendingReplay(tokio::sync::Notify);
impl transmog_session::ReplayExecutor for PendingReplay {
    fn execute(
        &self,
        _: transmog_session::ValidatedReplayRequest,
        _: transmog_core::intercept::ExchangeCancellation,
    ) -> transmog_session::BoxReplayFuture<'_> {
        self.0.notify_one();
        Box::pin(std::future::pending())
    }
    fn execute_file(
        &self,
        _: transmog_session::ValidatedReplayRequest,
        _: std::fs::File,
        _: u64,
        _: transmog_core::intercept::ExchangeCancellation,
    ) -> transmog_session::BoxReplayFuture<'_> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn interrupted_admission_becomes_unknown_without_resending() {
    let executor = Arc::new(PendingReplay(tokio::sync::Notify::new()));
    let h = replay_harness(executor.clone());
    let owner = h.grant(&[Permission::Send]);
    let input = json!({"sessionId":"main","instanceId":h.adapter.instance_id,"operationId":"one","request":{"method":"GET","url":"http://example.test/"}});
    let adapter = h.adapter.clone();
    let grant = owner.clone();
    let arguments = input.clone();
    let task = tokio::spawn(async move {
        adapter
            .handle(grant, "transmog_request_send", arguments)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), executor.0.notified())
        .await
        .unwrap();
    assert_eq!(
        h.call(
            &owner,
            "transmog_request_status",
            json!({"operationId":"one"})
        )
        .await
        .unwrap()["state"],
        "pending"
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        h.call(
            &owner,
            "transmog_request_status",
            json!({"operationId":"one"})
        )
        .await
        .unwrap()["state"],
        "outcome-unknown"
    );
    assert_eq!(
        h.adapter
            .handle(owner, "transmog_request_send", input)
            .await
            .unwrap_err()
            .code,
        "outcome_unknown"
    );
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn decoded_binary_context_is_lossless_and_incomplete_sources_are_not_documents() {
    let h = traffic(vec![0, 255, 128, 1, 2]);
    let owner = full(&h);
    let sent = send(&h, &owner, "binary").await;
    let page = h
        .call(
            &owner,
            "transmog_body_read",
            json!({"id":sent["id"],"boundary":"client-response","decodeContent":true}),
        )
        .await
        .unwrap();
    contract("transmog_body_read", &page);
    assert_eq!(page["encoding"], "base64");
    assert_eq!(context_bytes(&h, &owner, &page).await, h.body);
    let mut state = h.adapter.application.product_state();
    state.privacy.retain_response_bodies = false;
    h.adapter.application.save_product_state(state).unwrap();
    let omitted = send(&h, &owner, "omitted").await;
    assert!(
        h.call(
            &owner,
            "transmog_body_read",
            json!({"id":omitted["id"],"boundary":"client-response","decodeContent":true})
        )
        .await
        .is_err()
    );
    h.adapter.application.shutdown().await.unwrap();
}
