use crate::Connector;
use rmcp::{ServiceExt as _, model::CallToolRequestParams};

#[tokio::test]
async fn stdio_discovery_initializes_without_desktop_or_key_and_calls_explain_setup() {
    let root = tempfile::tempdir().unwrap();
    let (server_io, client_io) = tokio::io::duplex(128 * 1024);
    let connector = Connector::new(root.path().into(), String::new(), false);
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
    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name == "transmog_connect")
    );
    let response = client
        .call_tool(CallToolRequestParams::new("transmog_status"))
        .await
        .unwrap();
    assert_eq!(response.is_error, Some(true));
    assert_eq!(
        response.structured_content.unwrap()["error"]["code"],
        "unauthorized"
    );
    client.cancel().await.unwrap();
    server.await.unwrap();
}
