use crate::Connector;
use rmcp::{ServiceExt as _, model::CallToolRequestParams};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct LifecycleHandler(Arc<AtomicUsize>);
impl crate::Handler for LifecycleHandler {
    fn call(
        &self,
        _grant: crate::Grant,
        _operation: String,
        _input: serde_json::Value,
    ) -> crate::CallFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(serde_json::json!({"connected":true})) })
    }
}

fn lifecycle_access() -> (crate::AccessManager, String) {
    let access = crate::AccessManager::open(None).unwrap();
    let (_, key) = access
        .create(crate::GrantInput {
            name: "lifecycle test".into(),
            permissions: [crate::Permission::Inspect].into(),
            scope: crate::Scope::All,
            omit_sensitive_values: false,
            expires_at: None,
        })
        .unwrap();
    access.set_enabled(true).unwrap();
    (access, key.expose().to_owned())
}

struct ReadinessHandler {
    calls: Arc<AtomicUsize>,
    stalled: bool,
}
impl crate::Handler for ReadinessHandler {
    fn call(
        &self,
        _grant: crate::Grant,
        _operation: String,
        _input: serde_json::Value,
    ) -> crate::CallFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let stalled = self.stalled;
        Box::pin(async move {
            if stalled {
                std::future::pending::<()>().await;
            }
            Err(crate::Error::new(
                "permission_denied",
                "Readiness denied by fixture.",
            ))
        })
    }
}

#[tokio::test]
async fn startup_deadline_bounds_inflight_status_and_permanent_failures_are_not_retried() {
    for stalled in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let (access, key) = lifecycle_access();
        let calls = Arc::new(AtomicUsize::new(0));
        let _desktop = crate::LocalServer::start(
            root.path().into(),
            access,
            Arc::new(ReadinessHandler {
                calls: calls.clone(),
                stalled,
            }),
        )
        .await
        .unwrap();
        let connector = Connector::new(root.path().into(), key, true);
        // A short-lived owned child supplies a real process handle; its success
        // leaves readiness governed by the endpoint, as with a singleton handoff.
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            connector.wait_for_desktop(&mut child, Duration::from_millis(500)),
        )
        .await
        .unwrap()
        .unwrap_err();
        child.wait().await.unwrap();
        assert_eq!(
            error.code,
            if stalled {
                "startup_failed"
            } else {
                "permission_denied"
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn uninitialized_client_has_a_deadline_and_early_eof_is_clean() {
    let root = tempfile::tempdir().unwrap();
    let connector = Connector::new(root.path().into(), String::new(), false);
    let (server, _held_open) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(server);
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        connector
            .clone()
            .serve_io(reader, writer, Duration::from_millis(50)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code, "mcp_transport");
    let (server, client) = tokio::io::duplex(4096);
    drop(client);
    let (reader, writer) = tokio::io::split(server);
    tokio::time::timeout(
        Duration::from_secs(2),
        connector.serve_io(reader, writer, Duration::from_secs(30)),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn associated_desktop_stop_and_disablement_close_stdio_without_client_eof() {
    for disable in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (access, key) = lifecycle_access();
        let calls = Arc::new(AtomicUsize::new(0));
        let desktop = crate::LocalServer::start(
            root.path().into(),
            access.clone(),
            Arc::new(LifecycleHandler(calls.clone())),
        )
        .await
        .unwrap();
        let connector = Connector::new(root.path().into(), key, false);
        let (server_io, client_io) = tokio::io::duplex(128 * 1024);
        let server = tokio::spawn(async move {
            let (reader, writer) = tokio::io::split(server_io);
            connector
                .serve_io(reader, writer, Duration::from_secs(30))
                .await
        });
        let client = ().serve(client_io).await.unwrap();
        let result = client
            .call_tool(CallToolRequestParams::new("transmog_connect"))
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true));
        // Lifetime observation must not renew agent activity or issue status calls.
        tokio::time::sleep(Duration::from_millis(3200)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(access.active_agents(), [] as [serde_json::Value; 0]);
        if disable {
            access.set_enabled(false).unwrap();
        } else {
            drop(desktop);
        }
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        client.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn connector_never_rebinds_or_launches_again_when_discovery_is_replaced() {
    let root = tempfile::tempdir().unwrap();
    let (access, key) = lifecycle_access();
    let first_calls = Arc::new(AtomicUsize::new(0));
    let _first = crate::LocalServer::start(
        root.path().into(),
        access.clone(),
        Arc::new(LifecycleHandler(first_calls.clone())),
    )
    .await
    .unwrap();
    let connector = Connector::new(root.path().into(), key, true);
    connector
        .call("transmog_connect", serde_json::json!({}))
        .await
        .unwrap();
    let second_calls = Arc::new(AtomicUsize::new(0));
    let _second = crate::LocalServer::start(
        root.path().into(),
        access,
        Arc::new(LifecycleHandler(second_calls.clone())),
    )
    .await
    .unwrap();
    assert_eq!(
        connector
            .call("transmog_connect", serde_json::json!({}))
            .await
            .unwrap_err()
            .code,
        "not_running"
    );
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "One offline MCP session verifies discovery, static help and authorization failures."
)]
async fn stdio_discovery_initializes_without_desktop_or_key_and_calls_explain_setup() {
    let root = tempfile::tempdir().unwrap();
    let (server_io, client_io) = tokio::io::duplex(128 * 1024);
    let connector = Connector::new(root.path().into(), String::new(), true);
    let server = tokio::spawn(async move {
        let (reader, writer) = tokio::io::split(server_io);
        connector
            .serve_io(reader, writer, std::time::Duration::from_secs(30))
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let tools = client.list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), crate::tool_catalog().len());
    for tool in &tools.tools {
        let output = tool
            .output_schema
            .as_ref()
            .expect("Every tool advertises its result contract");
        assert!(
            output.contains_key("properties") || output.contains_key("anyOf"),
            "{} has no result fields",
            tool.name
        );
        if let Some(examples) = tool
            .input_schema
            .get("examples")
            .and_then(serde_json::Value::as_array)
        {
            for example in examples {
                crate::catalog::validation::validate(
                    &serde_json::Value::Object((*tool.input_schema).clone()),
                    example,
                )
                .unwrap();
            }
        }
    }
    let annotations = tools
        .tools
        .iter()
        .find(|tool| tool.name == "transmog_connect")
        .unwrap()
        .annotations
        .as_ref()
        .unwrap();
    assert_eq!(annotations.destructive_hint, Some(false));
    assert_eq!(annotations.open_world_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name == "transmog_connect")
    );
    let help_spec = crate::tool_catalog()
        .into_iter()
        .find(|spec| spec.name == "transmog_help")
        .unwrap();
    for section in crate::help::section_names() {
        let response = client
            .call_tool(
                CallToolRequestParams::new("transmog_help").with_arguments(
                    serde_json::json!({"topic":"scripts","section":section})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_ne!(response.is_error, Some(true));
        let help = response.structured_content.unwrap();
        crate::catalog::validation::validate(&help_spec.output_schema, &help).unwrap();
        assert_eq!(help["apiRevision"], 4);
        assert!(help["mcpSupport"].as_str().unwrap().contains("cannot"));
        assert_ne!(
            help["sections"].as_array().unwrap().as_slice(),
            &[] as &[serde_json::Value]
        );
    }
    let invalid_help = client
        .call_tool(
            CallToolRequestParams::new("transmog_help").with_arguments(
                serde_json::json!({"topic":"scripts","section":"invented"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(invalid_help.is_error, Some(true));
    let response = client
        .call_tool(CallToolRequestParams::new("transmog_status"))
        .await
        .unwrap();
    assert_eq!(response.is_error, Some(true));
    assert_eq!(
        response.structured_content.unwrap()["error"]["code"],
        "unauthorized"
    );
    let unknown = client
        .call_tool(CallToolRequestParams::new("transmog_missing"))
        .await
        .unwrap();
    assert_eq!(unknown.is_error, Some(true));
    assert_eq!(
        unknown.structured_content.unwrap()["error"]["code"],
        "unknown_tool"
    );
    client.cancel().await.unwrap();
    server.await.unwrap();
}
