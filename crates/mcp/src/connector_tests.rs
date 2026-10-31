use crate::Connector;
use rmcp::{ServiceExt as _, model::CallToolRequestParams};

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
        rmcp::ServiceExt::serve(connector, server_io)
            .await
            .unwrap()
            .waiting()
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
        assert_eq!(help["apiRevision"], 2);
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
