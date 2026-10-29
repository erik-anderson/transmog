//! Explicit session routing and capture lifecycle contracts.
use super::{ToolSpec, spec};
use serde_json::{Value, json};

#[allow(
    clippy::too_many_lines,
    reason = "The fixed declarative session tool catalog stays in protocol order."
)]
pub(super) fn complete(tools: &mut Vec<ToolSpec>) {
    let id = || json!({"type":"string","minLength":1,"maxLength":128});
    let instance = || json!({"type":"string","pattern":"^[0-9a-f]{32}$"});
    for (name, description, read, properties, required) in [
        (
            "transmog_session_create",
            "Create a private offline inspection session without opening a window or changing live collection. Idle expiry is 30 minutes; an open review window keeps the session alive.",
            false,
            json!({"name":{"type":"string","minLength":1,"maxLength":80}}),
            vec![],
        ),
        (
            "transmog_session_list",
            "List this key's private inspection sessions. The main workspace has sessionId main. Other keys' private sessions are inaccessible.",
            true,
            json!({}),
            vec![],
        ),
        (
            "transmog_session_renew",
            "Renew this key's private session for 30 minutes. Incoming traffic alone does not renew it.",
            false,
            json!({"sessionId":id()}),
            vec!["sessionId"],
        ),
        (
            "transmog_session_close",
            "End a private session and release its evidence. Stops no proxy or live-control lease. Unsaved human work requires action in its review window.",
            false,
            json!({"sessionId":id()}),
            vec!["sessionId"],
        ),
        (
            "transmog_capture_load",
            "Load a local TMCap, SAZ, HAR or Chromium NetLog JSON/JSONL file into a private session. Requires Load capture files. Poll the returned operation. Encrypted files pause for a password entered only in Transmog; never send passwords through MCP. Identical operationId retries never import twice.",
            false,
            json!({"sessionId":id(),"operationId":id(),"path":{"type":"string","minLength":1,"maxLength":4096}}),
            vec!["sessionId", "operationId", "path"],
        ),
        (
            "transmog_capture_list",
            "List complete imported-file groups, including empty captures. includeContext provides complete paged source context and requires all-traffic scope with unredacted header/body access.",
            true,
            json!({"sessionId":id(),"includeContext":{"type":"boolean"}}),
            vec!["sessionId"],
        ),
        (
            "transmog_capture_unload",
            "Unload a complete file group, markers, indexes and readers. Leaves source files, saved copies and independently retained replay results intact. Requires Load capture files. Use a new operationId and poll status.",
            false,
            json!({"sessionId":id(),"operationId":id(),"captureId":id()}),
            vec!["sessionId", "operationId", "captureId"],
        ),
        (
            "transmog_session_live_subscribe",
            "Subscribe a private session to references to shared live traffic, with optional exact request host/method filters. Requires Inspect traffic. includeExisting defaults false. Each network exchange and live rule runs once for all subscribers.",
            false,
            json!({"sessionId":id(),"includeExisting":{"type":"boolean"},"host":{"type":"string","minLength":1,"maxLength":253},"method":{"type":"string","minLength":1,"maxLength":32}}),
            vec!["sessionId"],
        ),
        (
            "transmog_session_live_unsubscribe",
            "Stop admitting live exchanges. Already admitted exchanges continue receiving response/stream updates on the shared engine.",
            false,
            json!({"sessionId":id()}),
            vec!["sessionId"],
        ),
        (
            "transmog_operation_status",
            "Read this key's capture operation state, progress and result. A waiting-password operation requires user action in Transmog and does not grant unredacted access.",
            true,
            json!({"operationId":id()}),
            vec!["operationId"],
        ),
        (
            "transmog_operation_cancel",
            "Cancel this key's pending capture operation. If publication already won, its authoritative state remains completed.",
            false,
            json!({"operationId":id()}),
            vec!["operationId"],
        ),
        (
            "transmog_rule_publish",
            "Copy an exact private rule revision into shared saved rules, disabled. Requires Edit shared rules. Testing private drafts changes no live traffic. Use shared live control to activate a published rule.",
            false,
            json!({"sessionId":id(),"id":id(),"expectedRevision":{"type":"integer","minimum":1}}),
            vec!["sessionId", "id", "expectedRevision"],
        ),
    ] {
        let mut properties = properties;
        properties["instanceId"] = instance();
        let mut required = required;
        required.push("instanceId");
        let mut tool = spec(name, description, read, properties, &required);
        tool.destructive = matches!(
            name,
            "transmog_session_close" | "transmog_capture_unload" | "transmog_operation_cancel"
        );
        tool.idempotent = read
            || matches!(
                name,
                "transmog_capture_load"
                    | "transmog_capture_unload"
                    | "transmog_session_live_unsubscribe"
                    | "transmog_operation_cancel"
            );
        tool.output_schema = result_schema(name);
        tools.push(tool);
    }
    for tool in tools.iter_mut().filter(|tool| inspection_tool(tool.name)) {
        tool.schema["properties"]["sessionId"] = id();
        tool.schema["required"]
            .as_array_mut()
            .expect("tool requirements")
            .push(json!("sessionId"));
        if let Some(examples) = tool.schema["examples"].as_array_mut() {
            for example in examples {
                example["sessionId"] = json!("main");
            }
        }
        if let Some(properties) = tool.output_schema["properties"].as_object_mut() {
            properties.insert("sessionId".into(), id());
        }
        if let Some(variants) = tool.output_schema["anyOf"].as_array_mut() {
            for variant in variants {
                if let Some(properties) = variant["properties"].as_object_mut() {
                    properties.insert("sessionId".into(), id());
                }
            }
        }
    }
    if let Some(status) = tools
        .iter_mut()
        .find(|tool| tool.name == "transmog_request_status")
    {
        status.output_schema["properties"]["result"]["anyOf"][0]["properties"]["sessionId"] = id();
    }
}

fn result_schema(name: &str) -> Value {
    let text = json!({"type":"string"});
    let flag = json!({"type":"boolean"});
    let session = json!({"type":"object","properties":{"sessionId":text,"name":text,"ownerId":text,"ownerName":text,"expiresAt":{"type":"integer"},"liveSubscribed":flag,"captureCount":{"type":"integer"},"reviewing":flag,"agentAccessEnded":flag,"needsHumanAction":flag,"liveError":{"type":["string","null"]}},"required":["sessionId","name","expiresAt","liveSubscribed","captureCount","reviewing","agentAccessEnded","needsHumanAction"]});
    match name {
        "transmog_session_list" => {
            json!({"type":"object","properties":{"sessions":{"type":"array","items":session,"maxItems":8},"mainSessionId":{"const":"main"}},"required":["sessions","mainSessionId"]})
        }
        "transmog_session_close" => {
            json!({"type":"object","properties":{"closed":{"const":true}},"required":["closed"]})
        }
        "transmog_capture_list" => {
            json!({"type":"object","properties":{"captures":{"type":"array","maxItems":16,"items":{"type":"object","properties":{"captureId":text,"name":text,"format":{"enum":["native","saz","har","netlog"]},"context":{"type":"object","description":"Paged complete saved source metadata; continue with transmog_context_read in this session."}},"required":["captureId","name","format"]}}},"required":["captures"]})
        }
        "transmog_capture_load"
        | "transmog_capture_unload"
        | "transmog_operation_status"
        | "transmog_operation_cancel" => {
            json!({"type":"object","properties":{"operationId":text,"sessionId":text,"cancelRequested":flag,"state":{"enum":["pending","waiting-password","completed","failed","cancelled"]},"completed":{"type":"integer","minimum":0},"total":{"type":"integer","minimum":0},"result":{"type":["object","null"]},"error":{"type":["object","null"]},"actionRequired":{"type":["string","null"]}},"required":["operationId","sessionId","state","completed","total","result","error","actionRequired"]})
        }
        "transmog_rule_publish" => {
            json!({"type":"object","properties":{"id":text,"revision":{"type":"integer"},"enabled":{"const":false}},"required":["id","revision","enabled"]})
        }
        _ => session,
    }
}

pub(crate) fn inspection_tool(name: &str) -> bool {
    matches!(
        name,
        "transmog_traffic_list"
            | "transmog_traffic_search"
            | "transmog_traffic_wait"
            | "transmog_exchange_get"
            | "transmog_exchanges_get"
            | "transmog_headers_read"
            | "transmog_body_read"
            | "transmog_body_extract"
            | "transmog_context_read"
            | "transmog_exchange_compare"
            | "transmog_stream_list"
            | "transmog_stream_read"
            | "transmog_request_prepare"
            | "transmog_request_send"
            | "transmog_request_status"
            | "transmog_rule_prepare"
            | "transmog_rule_test"
            | "transmog_rules_list"
            | "transmog_rule_get"
            | "transmog_rule_update"
    )
}
