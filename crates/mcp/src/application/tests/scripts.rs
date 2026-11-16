use super::*;

fn contract(result: &Value) {
    let tool = crate::tool_catalog()
        .into_iter()
        .find(|tool| tool.name == "transmog_scripts_status")
        .unwrap();
    crate::catalog::validation::validate(&tool.output_schema, result).unwrap();
}

#[tokio::test]
async fn script_registry_requires_inspection_and_global_scope_and_never_returns_source() {
    let h = Harness::new();
    h.adapter
        .application
        .save_script(serde_json::from_value(json!({
            "id":"diagnostic-script", "revision":7, "source":"source-must-stay-local",
            "language":"javascript", "sourceEdited":true, "handlers":["onRequestHead"],
            "prefilter":{"method":null,"host":"example.test","pathPrefix":"/sensitive-config"},
            "capabilities":{}, "priority":0,
            "limits":{"maxInputBodyBytes":1_048_576,"maxOutputBytes":1_048_576,"maxLogBytes":16_384,"maxHeapBytes":67_108_864,"maxDurationMs":100}
        })).unwrap())
        .unwrap();
    let mut grant = h.grant(&[Permission::Inspect]);
    assert_eq!(
        h.call(&grant, "transmog_scripts_status", json!({}))
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
    grant.scope = Scope::All;
    let redacted = h
        .call(
            &grant,
            "transmog_scripts_status",
            json!({"includeDiagnostics":true}),
        )
        .await
        .unwrap();
    contract(&redacted);
    assert_eq!(redacted["scripts"][0]["state"], "draft");
    assert_eq!(redacted["scripts"][0]["available"], Value::Null);
    assert!(!redacted.to_string().contains("sensitive-config"));
    grant
        .permissions
        .extend([Permission::ReadBodies, Permission::ReadCredentials]);
    let revealed = h
        .call(
            &grant,
            "transmog_scripts_status",
            json!({"includeDiagnostics":true,"limit":1}),
        )
        .await
        .unwrap();
    contract(&revealed);
    assert_eq!(revealed["scripts"][0]["revision"], 7);
    assert_eq!(revealed["scripts"][0]["handlers"], json!(["onRequestHead"]));
    assert!(revealed.to_string().contains("sensitive-config"));
    assert!(!revealed.to_string().contains("source-must-stay-local"));
    grant.permissions.remove(&Permission::Inspect);
    assert!(
        h.call(&grant, "transmog_scripts_status", json!({}))
            .await
            .is_err()
    );
}

#[test]
fn script_logs_are_exchange_scoped_and_initializer_output_requires_global_scope() {
    let h = Harness::new();
    let mut grant = h.grant(&[
        Permission::Inspect,
        Permission::ReadBodies,
        Permission::ReadCredentials,
    ]);
    let input = json!({"id":"allowed","scriptReports":[{"script":"test@1","phase":"RequestHead","annotations":[],"diagnostics":[],"detailsOmitted":false,"logs":[
        {"context":{"exchangeId":"allowed"},"records":[{"text":"in-scope"}]},
        {"context":{"exchangeId":"other"},"records":[{"text":"out-of-scope"}]},
        {"context":{"exchangeId":null},"records":[{"text":"initializer"}]}
    ]}]});
    let mut scoped = input.clone();
    project_script_reports(&mut scoped, &grant);
    assert!(scoped.to_string().contains("in-scope"));
    assert!(!scoped.to_string().contains("out-of-scope"));
    assert!(!scoped.to_string().contains("initializer"));
    assert_eq!(scoped["scriptReports"][0]["withheldLogRunCount"], 2);
    grant.scope = Scope::All;
    let mut global = input.clone();
    project_script_reports(&mut global, &grant);
    assert!(global.to_string().contains("initializer"));
    assert!(!global.to_string().contains("out-of-scope"));
    for permission in [Permission::ReadBodies, Permission::ReadCredentials] {
        let mut restricted = grant.clone();
        restricted.permissions.remove(&permission);
        let mut redacted = input.clone();
        project_script_reports(&mut redacted, &restricted);
        assert!(redacted["scriptReports"][0].get("logs").is_none());
    }
}

#[tokio::test]
async fn unvalidated_draft_admission_settings_follow_the_output_contract() {
    let h = Harness::new();
    let mut grant = h.grant(&[
        Permission::Inspect,
        Permission::ReadBodies,
        Permission::ReadCredentials,
    ]);
    grant.scope = Scope::All;
    let mut draft = json!({
        "id":"unfinished-script", "revision":1, "source":"unfinished source",
        "language":"javascript", "handlers":["onRequestHead"],
        "prefilter":{}, "capabilities":{}, "priority":0,
        "limits":{"maxInputBodyBytes":1_048_576,"maxOutputBytes":1_048_576,"maxLogBytes":16_384,"maxHeapBytes":67_108_864,"maxDurationMs":100}
    });
    for (field, settings) in [
        ("prefilter", json!({"pathPrefix":"x".repeat(20_000)})),
        ("requestBodyFilter", json!({"statuses":[65_535]})),
        (
            "responseBodyFilter",
            json!({"contentTypes":vec!["x".repeat(1_100);257]}),
        ),
    ] {
        draft[field] = settings;
        let saved: transmog_app::ScriptDraft = serde_json::from_value(draft.clone()).unwrap();
        let expected = json!(saved);
        h.adapter.application.save_script(saved).unwrap();
        let result = h
            .call(&grant, "transmog_scripts_status", json!({}))
            .await
            .unwrap();
        contract(&result);
        assert_eq!(result["scripts"][0]["state"], "draft");
        assert_eq!(result["scripts"][0][field], expected[field]);
    }
}

#[tokio::test]
async fn script_registry_context_rechecks_global_scope_and_disclosure() {
    let h = Harness::new();
    let mut grant = h.grant(&[
        Permission::Inspect,
        Permission::ReadBodies,
        Permission::ReadCredentials,
    ]);
    grant.scope = Scope::All;
    let context = h
        .adapter
        .context_json(
            &grant,
            &json!({"startup":"protected-initializer"}),
            vec![
                Permission::Inspect,
                Permission::ReadBodies,
                Permission::ReadCredentials,
            ],
            json!({"kind":"script-status"}),
            "http://localhost/".into(),
            None,
        )
        .unwrap();
    let input = json!({"contextId":context["contextId"]});
    assert!(
        h.call(&grant, "transmog_context_read", input.clone())
            .await
            .is_ok()
    );
    let mut restricted = grant.clone();
    restricted.scope = Scope::Hosts {
        hosts: vec!["localhost".into()],
    };
    assert!(
        h.call(&restricted, "transmog_context_read", input.clone())
            .await
            .is_err()
    );
    for permission in [Permission::ReadBodies, Permission::ReadCredentials] {
        let mut restricted = grant.clone();
        restricted.permissions.remove(&permission);
        assert!(
            h.call(&restricted, "transmog_context_read", input.clone())
                .await
                .is_err()
        );
    }
}
