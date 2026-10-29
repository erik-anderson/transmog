use super::*;

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
    h.adapter.maintain_sessions();
    assert!(review.access_ended());
    assert!(review.is_dirty());
    assert!(h.adapter.session_record_current(&owner, &id).is_err());
    review.set_dirty(false);
    h.adapter.maintain_sessions();
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
    h.adapter.maintain_sessions();
    assert_ne!(
        h.adapter.session_summaries(None),
        [] as [serde_json::Value; 0]
    );
    drop(review);
    h.adapter.expire_session_for_test(&id);
    h.adapter.maintain_sessions();
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
