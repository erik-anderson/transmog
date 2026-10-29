//! Public agent contracts, independent of desktop implementation types.
use super::{ToolSpec, spec};
use serde_json::{Value, json};
mod results;

fn text(max: usize) -> Value {
    json!({"type":"string","maxLength":max})
}
fn id() -> Value {
    json!({"type":"string","minLength":1,"maxLength":128})
}
fn uint(max: u64) -> Value {
    json!({"type":"integer","minimum":0,"maximum":max})
}
fn flag() -> Value {
    json!({"type":"boolean"})
}
fn choice(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}
fn array(item: Value, max: usize) -> Value {
    {
        let mut result = json!({"type":"array","maxItems":max});
        result["items"] = item;
        result
    }
}
fn object(properties: Value, required: &[&str]) -> Value {
    {
        let mut result = json!({"type":"object","required":required,"additionalProperties":false});
        result["properties"] = properties;
        result
    }
}
fn nullable(schema: Value) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        "anyOf".into(),
        Value::Array(vec![schema, json!({"type":"null"})]),
    )]))
}
fn instance() -> Value {
    json!({"type":"string","pattern":"^[0-9a-f]{32}$"})
}
fn boundary() -> Value {
    choice(&[
        "client-request",
        "upstream-request",
        "upstream-response",
        "client-response",
    ])
}
fn header() -> Value {
    object(
        json!({"name":text(256),"value":text(65_536)}),
        &["name", "value"],
    )
}
fn headers() -> Value {
    array(header(), 256)
}
fn defaulted(mut schema: Value, default: Value, description: &str) -> Value {
    schema["default"] = default;
    schema["description"] = json!(description);
    schema
}

pub(super) fn request() -> Value {
    object(
        json!({
            "method":text(32),"url":text(16_384),"headers":defaulted(headers(),json!([]),"Ordered fields; duplicates are preserved."),
            "body":defaulted(text(262_144),json!(""),"Inline UTF-8 text, or hexadecimal when bodyIsHex is true."),
            "bodyIsHex":defaulted(flag(),json!(false),"Interpret the inline body as hexadecimal bytes."),
            "bodySource":nullable(object(json!({"kind":{"const":"captured","type":"string"},"entryId":id()}), &["kind","entryId"])),
            "sourceEntryId":nullable(id()),"acknowledgeNonIdempotent":defaulted(flag(),json!(false),"Acknowledge sending a method that can have side effects."),
            "acknowledgeCredentials":defaulted(flag(),json!(false),"Acknowledge sending explicitly supplied credentials.")
        }),
        &["method", "url"],
    )
}

pub(super) fn rule_definition() -> Value {
    let common = json!({"kind":text(32),"name":json!({"type":"string","minLength":1,"maxLength":160}),"host":json!({"type":"string","minLength":1,"maxLength":255}),"method":nullable(text(32)),"pathPrefix":nullable(text(4096))});
    let operation = json!({"anyOf":[
        object(json!({"operation":{"const":"remove"},"name":text(256)}), &["operation","name"]),
        object(json!({"operation":choice(&["set","append","override-if-present"]),"name":text(256),"value":text(65_536)}), &["operation","name","value"])
    ]});
    let mut headers = common.clone();
    headers["kind"] = json!({"const":"headers"});
    headers["phase"] = choice(&["request", "response"]);
    headers["operations"] = array(operation, 64);
    headers["operations"]["minItems"] = json!(1);
    let mut replacement = common;
    replacement["kind"] = json!({"const":"response-text"});
    replacement["find"] = json!({"type":"string","minLength":1,"maxLength":65_536});
    replacement["replacement"] = text(65_536);
    replacement["occurrences"] = defaulted(
        choice(&["first", "all"]),
        json!("all"),
        "Non-overlapping literal matches.",
    );
    replacement["expectedMatches"] = nullable(uint(1_000_000));
    replacement["mediaType"] = text(128);
    replacement["maxBytes"] = defaulted(
        json!({"type":"integer","minimum":1,"maximum":1_048_576}),
        json!(1_048_576),
        "Complete decoded UTF-8 input and output bound.",
    );
    json!({"type":"object","anyOf":[object(headers,&["kind","name","host","phase","operations"]),object(replacement,&["kind","name","host","find","replacement","mediaType"])]})
}

fn query() -> Value {
    object(
        json!({
            "search":text(255),"method":text(32),"host":text(255),
            "statusMin":uint(599),"statusMax":uint(599),"durationMinMs":uint(u64::MAX),"durationMaxMs":uint(u64::MAX),
            "contentType":text(255),"process":text(255),"startedAfter":uint(u64::MAX),"startedBefore":uint(u64::MAX),
            "terminal":flag(),"sort":defaulted(choice(&["oldest","newest"]),json!("newest"),"Stable admission order captured at query creation.")
        }),
        &[],
    )
}

fn evidence() -> Value {
    let mut schema = object(
        json!({
            "instanceId":instance(),"id":id(),"requestNumber":text(32),"startedAt":uint(u64::MAX),
            "terminal":text(4096),"terminalState":text(64),
            "requests":array(json!({"type":"object","description":"Request head with boundary, target, method and ordered redacted headers."}),4),
            "responses":array(json!({"type":"object","description":"Response head with boundary, status and ordered redacted headers."}),4),
            "bodies":array(json!({"type":"object","description":"Retained body metadata; no payload bytes."}),4),
            "timing":json!({"type":"object","description":"Measured milestones, elapsed time and protocol evidence."}),
            "changes":json!({"type":"object","description":"Recorded header and body modifications, hooks and local responses."}),
            "transport":json!({"type":"object","description":"Route selection, attempts, diagnostics and stream lifetime."})
        }),
        &[
            "instanceId",
            "id",
            "requestNumber",
            "startedAt",
            "terminal",
            "terminalState",
        ],
    );
    schema["description"] = json!(
        "Compact exchange evidence. Only requested sections are returned. Bodies are metadata-only; use explicit body reads for payloads."
    );
    schema
}

fn page() -> Value {
    object(
        json!({"instanceId":instance(),"sessions":array(results::summary(),200),
        "totalMatched":uint(200_000),"nextCursor":nullable(id()),"unavailableIds":array(id(),200),"expiresAt":uint(u64::MAX),
        "coverage":text(256)}),
        &[
            "instanceId",
            "sessions",
            "totalMatched",
            "nextCursor",
            "unavailableIds",
            "expiresAt",
            "coverage",
        ],
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "Declarative public contracts are kept in protocol order."
)]
pub(super) fn complete(tools: &mut Vec<ToolSpec>) {
    for tool in tools.iter_mut() {
        tool.destructive = matches!(
            tool.name,
            "transmog_proxy_stop"
                | "transmog_rule_update"
                | "transmog_rule_activate"
                | "transmog_rule_disable"
                | "transmog_control_end"
                | "transmog_paused_decide"
        );
        tool.open_world = matches!(
            tool.name,
            "transmog_proxy_start"
                | "transmog_proxy_stop"
                | "transmog_request_send"
                | "transmog_rule_activate"
                | "transmog_rule_update"
                | "transmog_rule_disable"
                | "transmog_control_end"
                | "transmog_breakpoints_set"
                | "transmog_paused_decide"
        );
        tool.idempotent = tool.read_only
            || matches!(
                tool.name,
                "transmog_connect"
                    | "transmog_status"
                    | "transmog_request_send"
                    | "transmog_proxy_start"
                    | "transmog_proxy_stop"
            );
        tool.output_schema = json!({"type":"object","description":format!("Successful {} result. Failures use isError and {{error:{{code,message}}}}.", tool.name)});
        match tool.name {
            "transmog_traffic_list" => tool.output_schema = page(),
            "transmog_exchange_get" => tool.output_schema = evidence(),
            "transmog_rule_prepare" | "transmog_rule_update" => {
                tool.schema["examples"] = json!([{"instanceId":"00000000000000000000000000000000","rule":{"kind":"headers","name":"Add debug header","host":"example.test","phase":"request","operations":[{"operation":"set","name":"X-Debug","value":"1"}]}}]);
                if tool.name == "transmog_rule_update" {
                    tool.schema["examples"][0]["id"] = json!("shared-rule");
                    tool.schema["examples"][0]["expectedRevision"] = json!(1);
                }
            }
            _ => {}
        }
    }

    let mut batch = spec(
        "transmog_exchanges_get",
        "Inspect up to 20 scoped exchanges with the same section selection and a 512 KiB total result budget. Each item reports evidence or a safe per-item error; no bodies are returned.",
        true,
        json!({"instanceId":instance(),"ids":array(id(),20),"include":sections()}),
        &["instanceId", "ids"],
    );
    batch.schema["properties"]["ids"]["minItems"] = json!(1);
    batch.output_schema = object(
        json!({"instanceId":instance(),"items":array(json!({"anyOf":[evidence(),object(json!({"id":id(),"error":error_schema()}),&["id","error"])]}),20)}),
        &["instanceId", "items"],
    );
    tools.push(batch);
    let mut search = spec(
        "transmog_traffic_search",
        "Search only authorized retained metadata, headers and decoded bodies/stream text. Credential header values are excluded while redaction is enabled. Body search requires unredacted access. Returns a stable paged snapshot and explicit coverage counts. Does not change the human's current search.",
        true,
        json!({"instanceId":instance(),"query":query(),"pattern":text(4096),"mode":defaulted(choice(&["text","regex"]),json!("text"),"Bounded linear-time matching."),"caseSensitive":flag(),"fields":defaulted(array(choice(&["metadata","request-headers","response-headers","request-bodies","response-bodies"]),5),json!(["metadata"]),"Sources to search. Body fields also include retained stream text in that direction."),"limit":json!({"type":"integer","minimum":1,"maximum":200})}),
        &["instanceId", "pattern"],
    );
    search.output_schema = page();
    search.output_schema["properties"]["searchCoverage"] = object(
        json!({"examined":uint(200_000),"binaryBodies":uint(u64::MAX),"unavailableBodies":uint(u64::MAX)}),
        &["examined", "binaryBodies", "unavailableBodies"],
    );
    search.schema["properties"]["fields"]["minItems"] = json!(1);
    search.schema["properties"]["fields"]["uniqueItems"] = json!(true);
    tools.push(search);
    let mut extract = spec(
        "transmog_body_extract",
        "Extract JSON values by RFC 6901 JSON Pointer from a complete decoded body of at most 64 KiB. Requires unredacted access. Reports missing pointers separately and never parses an incomplete prefix as a full document.",
        true,
        json!({"instanceId":instance(),"id":id(),"boundary":boundary(),"pointers":array(text(4096),32)}),
        &["instanceId", "id", "boundary", "pointers"],
    );
    extract.output_schema = object(
        json!({"instanceId":instance(),"id":id(),"boundary":boundary(),"values":array(object(json!({"pointer":text(4096),"found":flag(),"value":{"description":"Selected JSON value; present only when found is true."}}), &["pointer","found"]),32),"coverage":text(256)}),
        &["instanceId", "id", "boundary", "values", "coverage"],
    );
    extract.schema["properties"]["pointers"]["minItems"] = json!(1);
    tools.push(extract);
    let mut prepare = spec(
        "transmog_request_prepare",
        "Prepare an editable request draft from the client-request or upstream-request boundary without sending. Omits Host, framing and connection headers; requires unredacted access before retaining credentials or a captured body. Review requirements, then send with a unique operationId.",
        true,
        json!({"instanceId":instance(),"id":id(),"boundary":defaulted(choice(&["client-request","upstream-request"]),json!("client-request"),"Source request boundary."),"includeCredentials":defaulted(flag(),json!(false),"Explicitly include captured credential values."),"includeBody":defaulted(flag(),json!(false),"Use a complete captured client-request body without copying it through agent context.")}),
        &["instanceId", "id"],
    );
    prepare.output_schema = object(
        json!({"instanceId":instance(),"request":request(),"droppedHeaders":array(object(json!({"name":text(256),"reason":text(256)}), &["name","reason"]),512),"requirements":array(text(256),8)}),
        &["instanceId", "request", "droppedHeaders", "requirements"],
    );
    tools.push(prepare);
    tools.push(spec("transmog_stream_list","List bounded WebSocket/SSE message metadata for an authorized exchange. Payloads and raw event bytes are read separately; metadata listing works while redaction is enabled.",true,json!({"instanceId":instance(),"id":id(),"offset":uint(u64::MAX),"limit":json!({"type":"integer","minimum":1,"maximum":200}),"sender":choice(&["client","server"]),"controls":flag()}),&["instanceId","id"]));
    tools.push(spec("transmog_stream_read","Read at most 64 KiB of one retained WebSocket/SSE payload as base64. Requires unredacted access. Retention loss and truncation remain explicit; offsets refer to retained decoded bytes.",true,json!({"instanceId":instance(),"id":id(),"sequence":uint(u64::MAX),"offset":uint(u64::MAX),"maxBytes":json!({"type":"integer","minimum":1,"maximum":65_536})}),&["instanceId","id","sequence"]));
    tools.push(spec("transmog_traffic_wait","Wait up to 30 seconds for authorized matching traffic admitted after afterRequestNumber. Checks existing matches before waiting, returns one bounded snapshot page, and does not block other tools or human controls. timeout is a successful empty outcome.",true,json!({"instanceId":instance(),"query":query(),"afterRequestNumber":json!({"type":"string","pattern":"^[0-9]{1,20}$"}),"timeoutMs":json!({"type":"integer","minimum":0,"maximum":30_000}),"limit":json!({"type":"integer","minimum":1,"maximum":200})}),&["instanceId","afterRequestNumber"]));
    for tool in tools.iter_mut() {
        results::complete(tool);
        if tool.name == "transmog_traffic_wait" {
            tool.output_schema = page();
            tool.output_schema["properties"]["timedOut"] = flag();
        }
        // Describe every scalar argument even where the contract has no default.
        if let Some(fields) = tool.schema["properties"].as_object_mut() {
            for (name, schema) in fields {
                if schema.get("description").is_none() {
                    schema["description"] = json!(argument_description(name));
                }
            }
        }
    }
    let evidence = tools
        .iter()
        .find(|tool| tool.name == "transmog_exchange_get")
        .expect("exchange tool")
        .output_schema
        .clone();
    let batch = tools
        .iter_mut()
        .find(|tool| tool.name == "transmog_exchanges_get")
        .expect("batch tool");
    batch.output_schema["properties"]["items"]["items"]["anyOf"][0] = evidence;
}

pub(super) fn sections() -> Value {
    defaulted(
        array(
            choice(&["headers", "bodies", "timing", "changes", "transport"]),
            5,
        ),
        json!(["headers", "bodies"]),
        "Allowlisted evidence sections. Bodies contain metadata only.",
    )
}
fn error_schema() -> Value {
    object(
        json!({"code":text(64),"message":text(4096)}),
        &["code", "message"],
    )
}
fn argument_description(name: &str) -> &str {
    match name {
        "instanceId" => "Current workspace instanceId from transmog_connect.",
        "expectedRevision" => "Exact reviewed saved revision; stale changes conflict.",
        "controlId" => "This key's current temporary control lease.",
        "operationId" => {
            "Unique send identifier. Reuse identical arguments after an uncertain outcome."
        }
        "cursor" => {
            "Opaque nextCursor from the previous page; bound to this key and its grant revision."
        }
        "query" => {
            "Additional filters within the key's approved scope. Time values are Unix milliseconds."
        }
        "rule" => {
            "Scoped header or literal response-text definition; shared edits preserve other existing match conditions."
        }
        "boundary" => "Explicit observed request or response boundary.",
        "offset" => "Zero-based position in retained evidence.",
        "seconds" => "Lease duration; defaults to 120 seconds.",
        "limit" => "Maximum items in this bounded response.",
        "id" => "Retained exchange or saved rule identifier named by this tool.",
        _ => "Explicit bounded input; see its type and allowed values.",
    }
}

pub(super) fn decision() -> Value {
    let bytes = || array(uint(255), 131_072);
    let wire_headers = || {
        array(
            object(
                json!({"name":text(256),"value":bytes()}),
                &["name", "value"],
            ),
            256,
        )
    };
    let head = object(
        json!({"method":text(32),"scheme":choice(&["http","https"]),"authority":text(255),"path_and_query":text(16_384),"headers":wire_headers()}),
        &["method", "scheme", "authority", "path_and_query", "headers"],
    );
    let response = object(
        json!({"status":uint(599),"headers":wire_headers()}),
        &["status", "headers"],
    );
    object(
        json!({"decisionId":uint(u64::MAX),"exchangeId":id(),"action":{"anyOf":[
            object(json!({"action":{"const":"continue"}}), &["action"]),
            object(json!({"action":{"const":"abort"},"reason":text(800)}), &["action","reason"]),
            object(json!({"action":{"const":"replace-body"},"body":bytes()}), &["action","body"]),
            object(json!({"action":{"const":"replace-request-head"},"head":head}), &["action","head"]),
            object(json!({"action":{"const":"replace-response-head"},"head":response}), &["action","head"])
        ]}}),
        &["decisionId", "exchangeId", "action"],
    )
}

pub(super) fn traffic_properties() -> Value {
    json!({"instanceId":instance(),"query":query(),"cursor":id(),"limit":defaulted(json!({"type":"integer","minimum":1,"maximum":200}),json!(50),"Maximum rows. Continue with cursor and limit only.")})
}
pub(super) fn sample() -> Value {
    object(
        json!({"method":text(32),"url":text(16_384),"requestHeaders":headers(),"responseStatus":uint(599),"responseHeaders":headers()}),
        &["method", "url"],
    )
}
pub(super) fn breakpoint_settings() -> Value {
    object(
        json!({"phases":array(choice(&["request-head","request-body","response-head","response-body"]),4),"bodyLimit":json!({"type":"integer","minimum":1,"maximum":131_072}),"maxPending":json!({"type":"integer","minimum":1,"maximum":8}),"timeoutMs":json!({"type":"integer","minimum":1,"maximum":30_000})}),
        &["phases", "bodyLimit", "maxPending", "timeoutMs"],
    )
}
