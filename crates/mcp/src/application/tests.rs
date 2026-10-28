use super::*;
use crate::GrantInput;
use transmog_app::AppConfig;

struct NoProxy;
impl ProxyHost for NoProxy {
    fn start(&self) -> CallFuture<'_> {
        Box::pin(async { Err(invalid("Proxy start is not used by rule tests.")) })
    }
}

struct Harness {
    _root: tempfile::TempDir,
    adapter: Arc<AgentApplication>,
    access: AccessManager,
}
impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let access = AccessManager::open(None).unwrap();
        access.set_enabled(true).unwrap();
        let application = Application::new(AppConfig {
            automation_path: Some(root.path().join("rules")),
            body_store: Some(transmog_app::BodyStoreConfig::product_default(
                root.path().join("bodies"),
            )),
            ..AppConfig::default()
        })
        .unwrap();
        let adapter = AgentApplication::new(
            application,
            access.clone(),
            root.path().into(),
            Arc::new(NoProxy),
        )
        .unwrap();
        Self {
            _root: root,
            adapter,
            access,
        }
    }
    fn grant(&self, permissions: &[Permission]) -> Grant {
        self.access
            .create(GrantInput {
                omit_sensitive_values: false,
                name: "Test agent".into(),
                permissions: permissions.iter().copied().collect(),
                scope: Scope::Hosts {
                    hosts: vec!["example.test".into()],
                },
                expires_at: None,
            })
            .unwrap()
            .0
    }
    async fn call(&self, grant: &Grant, operation: &str, mut input: Value) -> Result<Value, Error> {
        input["instanceId"] = json!(self.adapter.instance_id);
        self.adapter.handle(grant.clone(), operation, input).await
    }
    fn human_rule(&self, enabled: bool) -> Rule {
        let rule = rule("human-rule", enabled);
        parse(self.adapter.save_rule(rule, None).unwrap()).unwrap()
    }
}

fn rule(id: &str, enabled: bool) -> Rule {
    Rule {
        id: id.into(),
        display_name: Some("Shared header".into()),
        enabled,
        revision: 1,
        priority: 1,
        matcher: transmog_automation::RuleMatcher {
            host: Some("example.test".into()),
            ..Default::default()
        },
        request: transmog_automation::RequestActions {
            headers: vec![transmog_automation::HeaderOperation::Set(
                transmog_core::HeaderField::try_new("X-Collaboration", "first").unwrap(),
            )],
            ..Default::default()
        },
        response: transmog_automation::ResponseActions::default(),
    }
}

#[tokio::test]
async fn all_valid_keys_read_rules_without_an_edit_permission() {
    let h = Harness::new();
    let shared = h.human_rule(false);
    let reader = h.grant(&[Permission::Inspect]);
    let listed = h
        .call(&reader, "transmog_rules_list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["rules"][0]["id"], shared.id);
    let read = h
        .call(&reader, "transmog_rule_get", json!({"id":shared.id}))
        .await
        .unwrap();
    assert_eq!(
        read["rule"]["request"]["headers"].as_array().unwrap().len(),
        1
    );
    assert_eq!(read["editable"], false);
    assert_eq!(
        h.call(
            &reader,
            "transmog_rule_update",
            json!({"id":shared.id,"expectedRevision":shared.revision,"rule":shared})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    assert_eq!(
        h.call(
            &reader,
            "transmog_rule_disable",
            json!({"id":shared.id,"expectedRevision":shared.revision})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
}

#[tokio::test]
async fn shared_edits_guard_human_revisions_and_remain_after_revocation() {
    let h = Harness::new();
    let mut shared = h.human_rule(false);
    let writer = h.grant(&[Permission::EditSharedRules]);
    let initial = shared.clone();
    shared.display_name = Some("Human saved a newer name".into());
    let latest: Rule = parse(h.adapter.save_rule(shared, Some(initial.revision)).unwrap()).unwrap();
    assert_eq!(
        h.call(
            &writer,
            "transmog_rule_update",
            json!({"id":initial.id,"expectedRevision":initial.revision,"rule":initial})
        )
        .await
        .unwrap_err()
        .code,
        "conflict"
    );
    let mut edit = latest.clone();
    edit.display_name = Some("Agent and human collaboration".into());
    let saved = h
        .call(
            &writer,
            "transmog_rule_update",
            json!({"id":edit.id,"expectedRevision":latest.revision,"rule":edit}),
        )
        .await
        .unwrap();
    h.access
        .revoke(Some((&writer.id, writer.revision)))
        .unwrap();
    h.adapter.reconcile().await.unwrap();
    let retained = h.adapter.application.automation_status().rules;
    assert_eq!(
        retained[0].display_name.as_deref(),
        Some("Agent and human collaboration")
    );
    assert_eq!(retained[0].revision, saved["revision"].as_u64().unwrap());
}

#[tokio::test]
async fn enabled_shared_rule_edits_and_disable_require_live_permission() {
    let h = Harness::new();
    let shared = h.human_rule(true);
    let writer = h.grant(&[Permission::EditSharedRules]);
    assert_eq!(
        h.call(
            &writer,
            "transmog_rule_update",
            json!({"id":shared.id,"expectedRevision":shared.revision,"rule":shared})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    assert_eq!(
        h.call(
            &writer,
            "transmog_rule_disable",
            json!({"id":shared.id,"expectedRevision":shared.revision})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    let controller = h.grant(&[Permission::EditSharedRules, Permission::Modify]);
    let disabled = h
        .call(
            &controller,
            "transmog_rule_disable",
            json!({"id":shared.id,"expectedRevision":shared.revision}),
        )
        .await
        .unwrap();
    assert_eq!(disabled["enabled"], false);
}

#[tokio::test]
async fn temporary_rules_are_visible_to_humans_and_other_agents_but_controls_are_owned() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::PrepareChanges, Permission::Modify]);
    let other = h.grant(&[Permission::EditSharedRules, Permission::Modify]);
    let prepared = h
        .call(
            &owner,
            "transmog_rule_prepare",
            json!({"rule":rule("ignored",false)}),
        )
        .await
        .unwrap();
    assert_eq!(
        h.adapter.application.automation_status().rules[0].id,
        prepared["id"]
    );
    let read = h
        .call(&other, "transmog_rule_get", json!({"id":prepared["id"]}))
        .await
        .unwrap();
    assert!(!read["rule"].is_null());
    assert_eq!(read["editable"], false);
    assert_eq!(
        h.call(
            &other,
            "transmog_rule_disable",
            json!({"id":prepared["id"],"expectedRevision":prepared["revision"]})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    let control = h
        .call(&owner, "transmog_control_begin", json!({}))
        .await
        .unwrap();
    assert_eq!(
        h.call(
            &other,
            "transmog_control_end",
            json!({"controlId":control["id"]})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    h.call(&owner,"transmog_rule_activate",json!({"id":prepared["id"],"expectedRevision":prepared["revision"],"controlId":control["id"]})).await.unwrap();
    assert!(h.adapter.application.automation_status().rules[0].enabled);
    h.access.revoke(Some((&owner.id, owner.revision))).unwrap();
    h.adapter.reconcile().await.unwrap();
    assert!(!h.adapter.application.automation_status().rules[0].enabled);
    assert!(h.adapter.lock().controls.is_empty());
}

#[tokio::test]
async fn rule_reads_obey_disclosure_scope_and_instance_boundaries() {
    let h = Harness::new();
    let mut secret = rule("secret-rule", false);
    secret.request.headers = vec![transmog_automation::HeaderOperation::Set(
        transmog_core::HeaderField::try_new("Authorization", "Bearer retained-secret").unwrap(),
    )];
    h.adapter.save_rule(secret, None).unwrap();
    let mut reader = h.grant(&[Permission::Inspect]);
    let masked = h
        .call(&reader, "transmog_rule_get", json!({"id":"secret-rule"}))
        .await
        .unwrap();
    assert!(masked["rule"].is_null());
    assert!(!masked.to_string().contains("retained-secret"));
    reader.scope = Scope::Hosts {
        hosts: vec!["unrelated.test".into()],
    };
    assert_eq!(
        h.call(&reader, "transmog_rules_list", json!({}))
            .await
            .unwrap()["rules"],
        json!([])
    );
    reader.scope = Scope::Requests {
        instance_id: "0".repeat(32),
        ids: vec!["old-id".into()],
    };
    let mut broad = rule("broad", false);
    broad.matcher.host = None;
    h.adapter.save_rule(broad, None).unwrap();
    assert_eq!(
        h.call(&reader, "transmog_rules_list", json!({}))
            .await
            .unwrap()["rules"],
        json!([])
    );
}

#[test]
fn schema_validation_rejects_unknown_fields_wrong_types_and_unbounded_pages() {
    assert!(validate_arguments("transmog_traffic_list", &json!({"limit":201})).is_err());
    assert!(validate_arguments("transmog_traffic_list", &json!({"limit":"100"})).is_err());
    assert!(
        validate_arguments(
            "transmog_status",
            &json!({"accessKey":"do not accept inline keys"})
        )
        .is_err()
    );
    assert!(
        validate_arguments(
            "transmog_body_read",
            &json!({"instanceId":"0".repeat(32),"id":"1","boundary":"unknown"})
        )
        .is_err()
    );
}

#[tokio::test]
async fn keeping_an_agent_rule_preserves_the_humans_pause_and_survives_lease_cleanup() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::PrepareChanges, Permission::Modify]);
    let prepared = h
        .call(
            &owner,
            "transmog_rule_prepare",
            json!({"rule":rule("ignored",false)}),
        )
        .await
        .unwrap();
    let control = h
        .call(&owner, "transmog_control_begin", json!({}))
        .await
        .unwrap();
    let active=h.call(&owner,"transmog_rule_activate",json!({"id":prepared["id"],"expectedRevision":prepared["revision"],"controlId":control["id"]})).await.unwrap();
    let status = h.adapter.application.automation_status();
    h.adapter
        .application
        .set_autoresponses_enabled(false, status.generation)
        .unwrap();
    h.adapter
        .keep_rule(
            active["id"].as_str().unwrap(),
            active["revision"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    h.adapter.end_all().await.unwrap();
    let kept = h.adapter.application.automation_status();
    assert!(!kept.autoresponses_enabled);
    assert_eq!(kept.rules.len(), 1);
    assert!(!kept.rules[0].id.starts_with("mcp-"));
    assert!(kept.rules[0].enabled);
}

#[tokio::test]
async fn reopening_native_rules_disables_unfinished_temporary_changes_before_mcp_starts() {
    let h = Harness::new();
    let owner = h.grant(&[Permission::PrepareChanges]);
    let prepared = h
        .call(
            &owner,
            "transmog_rule_prepare",
            json!({"rule":rule("ignored",false)}),
        )
        .await
        .unwrap();
    let enabled = h
        .adapter
        .set_rule_enabled(
            prepared["id"].as_str().unwrap(),
            prepared["revision"].as_u64().unwrap(),
            true,
        )
        .unwrap();
    assert_eq!(
        h.call(
            &owner,
            "transmog_rule_prepare",
            json!({"rule":enabled,"expectedRevision":enabled["revision"]})
        )
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    let reopened = Application::new(AppConfig {
        automation_path: Some(h.adapter.root.join("rules")),
        ..AppConfig::default()
    })
    .unwrap();
    assert!(!reopened.automation_status().rules[0].enabled);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Exercise a complete real proxy exchange across the MCP transport."
)]
async fn real_loopback_proxy_exposes_scoped_original_and_effective_evidence_through_mcp() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let h = Harness::new();
    let mut preferences = h.adapter.application.product_state();
    preferences.preferences.decrypt_https = false;
    preferences.preferences.configure_system_proxy = false;
    h.adapter
        .application
        .save_product_state(preferences)
        .unwrap();
    h.adapter
        .application
        .start_proxy(transmog_app::ProxyStartRequest::default(), None)
        .await
        .unwrap();
    let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let producer = tokio::spawn(async move {
        let (mut socket, _) = origin.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nSet-Cookie: original-secret\r\nConnection: close\r\n\r\nbody-test").await.unwrap();
    });
    let (grant, key) = h
        .access
        .create(GrantInput {
            omit_sensitive_values: false,
            name: "Loopback test".into(),
            permissions: [
                Permission::Inspect,
                Permission::ReadBodies,
                Permission::PrepareChanges,
                Permission::Modify,
            ]
            .into_iter()
            .collect(),
            scope: Scope::Hosts {
                hosts: vec!["127.0.0.1".into()],
            },
            expires_at: None,
        })
        .unwrap();
    let server = crate::LocalServer::start_for_instance(
        h.adapter.root.clone(),
        h.adapter.instance_id.clone(),
        h.access.clone(),
        h.adapter.clone(),
    )
    .await
    .unwrap();
    let client = crate::LocalClient::new(h.adapter.root.clone());
    let mut rule = rule("draft", false);
    rule.matcher.host = Some("127.0.0.1".into());
    rule.response.headers = std::mem::take(&mut rule.request.headers);
    let prepared = h
        .call(&grant, "transmog_rule_prepare", json!({"rule":rule}))
        .await
        .unwrap();
    let control = h
        .call(&grant, "transmog_control_begin", json!({}))
        .await
        .unwrap();
    h.call(&grant,"transmog_rule_activate",json!({"id":prepared["id"],"expectedRevision":prepared["revision"],"controlId":control["id"]})).await.unwrap();
    let mut socket =
        tokio::net::TcpStream::connect(h.adapter.application.status().listener.unwrap())
            .await
            .unwrap();
    socket.write_all(format!("GET http://{destination}/evidence HTTP/1.1\r\nHost: {destination}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    producer.await.unwrap();
    let page = client
        .call(key.expose(), "transmog_traffic_list", json!({}))
        .await
        .unwrap();
    assert_eq!(page["sessions"].as_array().unwrap().len(), 1);
    let id = &page["sessions"][0]["id"];
    let detail = client
        .call(
            key.expose(),
            "transmog_exchange_get",
            json!({"instanceId":h.adapter.instance_id,"id":id}),
        )
        .await
        .unwrap();
    assert!(!detail.to_string().contains("original-secret"));
    let heads = detail["responses"].as_array().unwrap();
    assert!(heads.iter().any(|head| {
        head["boundary"] == "upstream-response"
            && !head["headers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|header| header["name"] == "X-Collaboration")
    }));
    assert!(heads.iter().any(|head| {
        head["boundary"] == "client-response"
            && head["headers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|header| header["name"] == "X-Collaboration")
    }));
    let (_, outsider) = h
        .access
        .create(GrantInput {
            omit_sensitive_values: false,
            name: "Other host".into(),
            permissions: [Permission::Inspect].into_iter().collect(),
            scope: Scope::Hosts {
                hosts: vec!["other.test".into()],
            },
            expires_at: None,
        })
        .unwrap();
    assert_eq!(
        client
            .call(outsider.expose(), "transmog_traffic_list", json!({}))
            .await
            .unwrap()["totalMatched"],
        0
    );
    drop(server);
    h.adapter.end_all().await.unwrap();
    h.adapter.application.shutdown().await.unwrap();
}

struct ResponseFixture(std::sync::atomic::AtomicUsize);

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Keep one original-value consent and evidence lifecycle together."
)]
async fn protected_evidence_requires_a_human_decision_and_is_hidden_again_after_revocation() {
    let root = tempfile::tempdir().unwrap();
    let access = AccessManager::open(None).unwrap();
    access.set_enabled(true).unwrap();
    let application = Application::new(AppConfig {
        replay_executor: Some(Arc::new(ResponseFixture(
            std::sync::atomic::AtomicUsize::new(0),
        ))),
        ..AppConfig::default()
    })
    .unwrap();
    let adapter = AgentApplication::new(
        application,
        access.clone(),
        root.path().into(),
        Arc::new(NoProxy),
    )
    .unwrap();
    let h = Harness {
        _root: root,
        adapter,
        access,
    };
    let mut config:GrantInput=serde_json::from_value(json!({"name":"Protected agent","permissions":["inspect","send","read-bodies","read-credentials"],"scope":{"kind":"hosts","hosts":["example.test"]},"expiresAt":null})).unwrap();
    assert!(config.omit_sensitive_values);
    config.name = "Protected agent".into();
    let (_, key) = h.access.create(config).unwrap();
    let _server = crate::LocalServer::start_for_instance(
        h.adapter.root.clone(),
        h.adapter.instance_id.clone(),
        h.access.clone(),
        h.adapter.clone(),
    )
    .await
    .unwrap();
    let client = crate::LocalClient::new(h.adapter.root.clone());
    let sent=client.call(key.expose(),"transmog_request_send",json!({"instanceId":h.adapter.instance_id,"operationId":"protected-send","request":{"method":"GET","url":"http://example.test/path","headers":[],"body":"","acknowledgeNonIdempotent":false,"acknowledgeCredentials":false}})).await.unwrap();
    let input =
        json!({"instanceId":h.adapter.instance_id,"id":sent["id"],"boundary":"client-response"});
    assert_eq!(
        client
            .call(key.expose(), "transmog_body_read", input.clone())
            .await
            .unwrap_err()
            .code,
        "sensitive_access_required"
    );
    let detail = client
        .call(
            key.expose(),
            "transmog_exchange_get",
            json!({"instanceId":h.adapter.instance_id,"id":sent["id"]}),
        )
        .await
        .unwrap();
    assert!(!detail.to_string().contains("retained-cookie-secret"));
    let pending = client
        .call(
            key.expose(),
            "transmog_sensitive_access_request",
            json!({"reason":"Read the original failure response."}),
        )
        .await
        .unwrap();
    assert_eq!(pending["headersAllowed"], false);
    let id = pending["request"]["id"].as_str().unwrap();
    h.access.decide_sensitive_access(id, "minute").unwrap();
    let body = client
        .call(key.expose(), "transmog_body_read", input.clone())
        .await
        .unwrap();
    assert_eq!(
        STANDARD.decode(body["data"].as_str().unwrap()).unwrap(),
        b"retained-body-secret"
    );
    let detail = client
        .call(
            key.expose(),
            "transmog_exchange_get",
            json!({"instanceId":h.adapter.instance_id,"id":sent["id"]}),
        )
        .await
        .unwrap();
    assert!(detail.to_string().contains("retained-cookie-secret"));
    h.access.revoke_sensitive_access(id).unwrap();
    assert_eq!(
        client
            .call(key.expose(), "transmog_body_read", input)
            .await
            .unwrap_err()
            .code,
        "sensitive_access_required"
    );
    assert!(
        client
            .call(
                key.expose(),
                "transmog_sensitive_access_request",
                json!({"reason":"Bypass", "choice":"always"})
            )
            .await
            .is_err()
    );
}
impl transmog_session::ReplayExecutor for ResponseFixture {
    fn execute(
        &self,
        _: transmog_session::ValidatedReplayRequest,
        _: transmog_core::intercept::ExchangeCancellation,
    ) -> transmog_session::BoxReplayFuture<'_> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async {
            Ok(transmog_session::ReplayResponse {
                source_version: transmog_core::HttpLegVersion::Http1,
                status: 200,
                body: bytes::Bytes::from_static(b"retained-body-secret"),
                headers: transmog_core::HeaderBlock::from_fields(vec![
                    transmog_core::HeaderField::try_new("Set-Cookie", "retained-cookie-secret")
                        .unwrap(),
                ]),
            })
        })
    }
    fn execute_file(
        &self,
        _: transmog_session::ValidatedReplayRequest,
        _: std::fs::File,
        _: u64,
        _: transmog_core::intercept::ExchangeCancellation,
    ) -> transmog_session::BoxReplayFuture<'_> {
        Box::pin(async {
            Err(transmog_session::ReplayExecutionError::new(
                "File sends must not reach the executor",
            ))
        })
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Exercise one complete send and evidence lifecycle."
)]
async fn send_retry_is_deduplicated_and_evidence_reads_check_body_and_scope() {
    let root = tempfile::tempdir().unwrap();
    let access = AccessManager::open(None).unwrap();
    access.set_enabled(true).unwrap();
    let executor = Arc::new(ResponseFixture(std::sync::atomic::AtomicUsize::new(0)));
    let application = Application::new(AppConfig {
        replay_executor: Some(executor.clone()),
        ..AppConfig::default()
    })
    .unwrap();
    let adapter = AgentApplication::new(
        application,
        access.clone(),
        root.path().into(),
        Arc::new(NoProxy),
    )
    .unwrap();
    let h = Harness {
        _root: root,
        adapter,
        access,
    };
    let sender = h.grant(&[
        Permission::Send,
        Permission::Inspect,
        Permission::ReadBodies,
    ]);
    let request = json!({"method":"GET","url":"http://example.test/path","headers":[],"body":"","acknowledgeNonIdempotent":false,"acknowledgeCredentials":false});
    let sent = h
        .call(
            &sender,
            "transmog_request_send",
            json!({"operationId":"one","request":request}),
        )
        .await
        .unwrap();
    let repeated = h
        .call(
            &sender,
            "transmog_request_send",
            json!({"operationId":"one","request":request}),
        )
        .await
        .unwrap();
    assert_eq!(sent, repeated);
    assert_eq!(executor.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    let mut changed = request.clone();
    changed["url"] = json!("http://example.test/different");
    assert_eq!(
        h.call(
            &sender,
            "transmog_request_send",
            json!({"operationId":"one","request":changed})
        )
        .await
        .unwrap_err()
        .code,
        "conflict"
    );
    let read = h
        .call(&sender, "transmog_exchange_get", json!({"id":sent["id"]}))
        .await
        .unwrap();
    assert!(!read.to_string().contains("retained-cookie-secret"));
    assert!(!read.to_string().contains("retained-body-secret"));
    let body = h
        .call(
            &sender,
            "transmog_body_read",
            json!({"id":sent["id"],"boundary":"client-response","maxBytes":8}),
        )
        .await
        .unwrap();
    assert_eq!(
        STANDARD.decode(body["data"].as_str().unwrap()).unwrap(),
        b"retained"
    );
    assert_eq!(body["nextOffset"], "8");
    let reader = h.grant(&[Permission::Inspect]);
    assert_eq!(
        h.call(
            &reader,
            "transmog_body_read",
            json!({"id":sent["id"],"boundary":"client-response"})
        )
        .await
        .unwrap_err()
        .code,
        "sensitive_access_required"
    );
    let mut outsider = reader.clone();
    outsider.scope = Scope::Hosts {
        hosts: vec!["other.test".into()],
    };
    assert_eq!(
        h.call(&outsider, "transmog_exchange_get", json!({"id":sent["id"]}))
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(
        h.adapter
            .handle(
                reader,
                "transmog_exchange_get",
                json!({"instanceId":"0".repeat(32),"id":sent["id"]})
            )
            .await
            .unwrap_err()
            .code,
        "stale_instance"
    );
}
