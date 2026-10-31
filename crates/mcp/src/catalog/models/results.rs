//! Discoverable response models. Native diagnostic extensions are explicitly open.
use super::{
    ToolSpec, Value, array, boundary, choice, flag, id, instance, json, nullable, object,
    rule_definition, text, uint,
};

fn open(properties: Value, description: &str) -> Value {
    {
        let mut result =
            json!({"type":"object","additionalProperties":true,"description":description});
        result["properties"] = properties;
        result
    }
}
fn strings() -> Value {
    array(text(16_384), 1024)
}
fn count() -> Value {
    uint(u64::MAX)
}
fn maybe_text() -> Value {
    nullable(text(16_384))
}
fn caller() -> Value {
    object(
        json!({"kind":choice(&["local-process","local-unknown","remote"]),"processName":maybe_text(),"processId":nullable(count())}),
        &["kind", "processName", "processId"],
    )
}

pub(super) fn summary() -> Value {
    object(
        json!({"id":id(),"requestNumber":text(20),"method":text(32),"url":text(16_384),"host":text(255),"status":nullable(uint(599)),"durationMs":nullable(count()),"contentType":maybe_text(),"startedAt":count(),"terminal":text(128),"caller":caller(),"streamProtocol":nullable(choice(&["web-socket","sse"])),"requestBytes":count(),"responseBytes":count(),"loss":flag()}),
        &[
            "id",
            "requestNumber",
            "method",
            "url",
            "host",
            "status",
            "durationMs",
            "contentType",
            "startedAt",
            "terminal",
            "caller",
            "streamProtocol",
            "requestBytes",
            "responseBytes",
            "loss",
        ],
    )
}

fn body_metadata() -> Value {
    object(
        json!({"lengthKnown":flag(),"exchangeId":id(),"boundary":boundary(),"observedBytes":count(),"wireBodyBytes":nullable(count()),"retainedBytes":count(),"availability":text(64),"mediaType":maybe_text(),"charset":maybe_text(),"contentCodings":strings(),"sha256":maybe_text(),"reason":maybe_text()}),
        &[
            "lengthKnown",
            "exchangeId",
            "boundary",
            "observedBytes",
            "wireBodyBytes",
            "retainedBytes",
            "availability",
            "mediaType",
            "charset",
            "contentCodings",
            "sha256",
            "reason",
        ],
    )
}
fn header_view() -> Value {
    object(
        json!({"index":count(),"valueBytes":nullable(count()),"fieldBytes":nullable(count()),"name":text(4096),"value":text(16_384),"binary":flag(),"sensitive":flag()}),
        &[
            "index",
            "valueBytes",
            "fieldBytes",
            "name",
            "value",
            "binary",
            "sensitive",
        ],
    )
}
fn header_summary() -> Value {
    object(
        json!({"totalFields":count(),"valueBytes":nullable(count()),"serializedBytes":nullable(count()),"authorization":text(64),"proxyAuthorization":text(64)}),
        &[
            "totalFields",
            "valueBytes",
            "serializedBytes",
            "authorization",
            "proxyAuthorization",
        ],
    )
}
fn head() -> Value {
    object(
        json!({"summary":header_summary(),"boundary":boundary(),"method":maybe_text(),"target":maybe_text(),"status":nullable(uint(599)),"protocol":text(64),"headers":array(header_view(),512)}),
        &[
            "summary", "boundary", "method", "target", "status", "protocol", "headers",
        ],
    )
}
fn scope() -> Value {
    json!({"anyOf":[object(json!({"kind":{"const":"all"}}), &["kind"]),object(json!({"kind":{"const":"hosts"},"hosts":array(text(255),64)}), &["kind","hosts"]),object(json!({"kind":{"const":"requests"},"instanceId":instance(),"ids":array(id(),4096)}), &["kind","instanceId","ids"])]})
}
fn grant() -> Value {
    object(
        json!({"id":id(),"name":text(160),"permissions":array(choice(&["inspect","load-captures","read-bodies","read-credentials","collect","prepare-changes","edit-shared-rules","modify","send"]),9),"scope":scope(),"expiresAt":nullable(count()),"omitSensitiveValues":flag()}),
        &[
            "id",
            "name",
            "permissions",
            "scope",
            "expiresAt",
            "omitSensitiveValues",
        ],
    )
}
fn disclosure() -> Value {
    object(
        json!({"omitSensitiveValues":flag(),"headersAllowed":flag(),"bodiesAllowed":flag(),"request":nullable(object(json!({"id":id(),"grantId":id(),"revision":count(),"name":text(160),"scope":scope(),"reason":text(800),"state":text(32),"requestedAt":count(),"expiresAt":count(),"remainingSeconds":count()}), &["id","grantId","revision","name","scope","reason","state","requestedAt","expiresAt","remainingSeconds"])),"instructions":text(4096)}),
        &[
            "omitSensitiveValues",
            "headersAllowed",
            "bodiesAllowed",
            "request",
            "instructions",
        ],
    )
}
fn app_status() -> Value {
    object(
        json!({"lifecycle":text(64),"listener":maybe_text(),"summary":text(4096),"hostRestorePending":flag()}),
        &["lifecycle", "listener", "summary", "hostRestorePending"],
    )
}
fn native_rule() -> Value {
    open(
        json!({"id":id(),"revision":count(),"enabled":flag(),"displayName":maybe_text(),"priority":{"type":"integer"},"matcher":open(json!({"host":maybe_text(),"method":maybe_text(),"pathPrefix":maybe_text(),"scheme":maybe_text(),"port":nullable(uint(65_535)),"query":maybe_text(),"requestHeaders":array(open(json!({"name":text(256),"condition":{"description":"Native byte predicate; read-only evidence."}}),"Native header condition."),256),"responseHeaders":array(json!({"type":"object"}),256)}),"Complete native match conditions, preserved during shared authoring edits."),"request":open(json!({"headers":array(json!({"type":"object"}),256),"abortReason":maybe_text(),"discardBody":flag(),"replaceBody":nullable(array(uint(255),1_048_576)),"responseAsset":maybe_text()}),"Complete native request actions."),"response":open(json!({"headers":array(json!({"type":"object"}),256),"abortReason":maybe_text(),"discardBody":flag(),"replaceBody":nullable(array(uint(255),1_048_576)),"textTransform":nullable(open(json!({"find":text(65_536),"replacement":text(65_536),"mediaType":text(128),"encoding":text(64),"maxBytes":count(),"occurrences":choice(&["first","all"]),"expectedMatches":nullable(count())}),"Native literal transform."))}),"Complete native response actions.")}),
        "Native saved rule evidence. Use definition from rule_get as the authoring input, retaining the separately returned revision.",
    )
}
fn rule_summary() -> Value {
    object(
        json!({"sessionId":id(),"id":id(),"revision":count(),"name":maybe_text(),"enabled":flag(),"host":maybe_text(),"method":maybe_text(),"pathPrefix":maybe_text(),"priority":{"type":"integer"},"effects":open(json!({"requestHeaders":count(),"responseHeaders":count(),"requestBodyReplacement":flag(),"responseBodyReplacement":flag(),"responseTextReplacement":flag(),"savedResponse":flag(),"userAgentOverride":flag(),"requestAbort":flag(),"responseAbort":flag(),"discardRequestBody":flag(),"discardResponseBody":flag()}),"Content-free native rule side effects."),"temporaryAgentRule":flag()}),
        &[
            "id",
            "revision",
            "name",
            "enabled",
            "host",
            "method",
            "pathPrefix",
            "priority",
            "effects",
            "temporaryAgentRule",
        ],
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "Declarative result contracts are kept in protocol order."
)]
pub(super) fn complete(tool: &mut ToolSpec) {
    match tool.name {
        "transmog_help" => {
            let phase = object(
                json!({"handler":text(64),"input":text(256),"decisions":text(1024)}),
                &["handler", "input", "decisions"],
            );
            let example = object(
                json!({"title":text(128),"settings":text(4096),"code":text(65_536)}),
                &["title", "settings", "code"],
            );
            let section = object(
                json!({"id":text(64),"title":text(128),"paragraphs":array(text(4096),32),"phases":array(phase,6),"examples":array(example,16)}),
                &["id", "title", "paragraphs", "phases", "examples"],
            );
            tool.output_schema = object(
                json!({"topic":choice(&["scripts"]),"apiRevision":uint(u32::MAX.into()),"mcpSupport":text(4096),"failureCategories":array(text(64),32),"sections":array(section,16)}),
                &[
                    "topic",
                    "apiRevision",
                    "mcpSupport",
                    "failureCategories",
                    "sections",
                ],
            );
        }

        "transmog_connect" | "transmog_status" => {
            tool.output_schema = object(
                json!({"connected":flag(),"instanceId":instance(),"grant":grant(),"sensitiveAccess":disclosure(),"proxy":app_status(),"recordingActive":flag(),"limits":object(json!({"pageSize":uint(200),"bodyReadBytes":uint(65_536),"controlLeaseSeconds":uint(600)}), &["pageSize","bodyReadBytes","controlLeaseSeconds"])}),
                &[
                    "connected",
                    "instanceId",
                    "grant",
                    "sensitiveAccess",
                    "proxy",
                    "recordingActive",
                    "limits",
                ],
            );
        }
        "transmog_sensitive_access_request" | "transmog_sensitive_access_status" => {
            tool.output_schema = disclosure();
        }
        "transmog_proxy_start" | "transmog_proxy_stop" => tool.output_schema = app_status(),
        "transmog_recording_start" => {
            tool.output_schema = object(
                json!({"recording":flag(),"artifact":text(255)}),
                &["recording", "artifact"],
            );
        }
        "transmog_recording_stop" => {
            tool.output_schema = object(
                json!({"sealed":flag(),"artifact":maybe_text()}),
                &["sealed", "artifact"],
            );
        }
        "transmog_headers_read" => {
            tool.output_schema = object(
                json!({"headers":array(open(json!({"name":text(524_288),"value":text(524_288),"encoding":text(64),"sensitive":flag()}),"Complete ordered field value."),200),"totalHeaders":count(),"offset":count(),"nextOffset":nullable(count()),"context":super::context_ref(),"coverage":text(256)}),
                &[
                    "headers",
                    "totalHeaders",
                    "offset",
                    "nextOffset",
                    "context",
                    "coverage",
                ],
            );
        }
        "transmog_body_read" => {
            tool.output_schema = json!({"type":"object","anyOf":[
                object(json!({"metadata":body_metadata(),"offset":text(20),"totalBytes":text(20),"encoding":choice(&["base64"]),"data":text(90_000),"nextOffset":nullable(text(20))}), &["metadata","offset","totalBytes","encoding","data","nextOffset"]),
                super::context_page()
            ]});
        }
        "transmog_exchange_get" => {
            tool.output_schema["properties"]["context"] = super::context_ref();
            tool.output_schema["properties"]["inlineComplete"] = flag();
            tool.output_schema["properties"]["terminalDetailsAvailable"] = flag();
            tool.output_schema["properties"]["requests"] = array(head(), 4);
            tool.output_schema["properties"]["responses"] = array(head(), 4);
            tool.output_schema["properties"]["bodies"] = array(body_metadata(), 4);
            tool.output_schema["properties"]["timing"] = open(
                json!({"points":array(object(json!({"milestone":text(64),"unixMillis":count(),"offsetMicros":{"type":"integer"}}), &["milestone","unixMillis","offsetMicros"]),32),"work":array(open(json!({"kind":text(128),"label":text(1024),"beganOffsetMicros":{"type":"integer"},"endedOffsetMicros":{"type":"integer"},"busyNanos":count(),"calls":count()}),"Measured local work intervals."),128),"protocols":array(open(json!({"boundary":boundary(),"version":text(64),"reason":maybe_text()}),"Observed message protocol."),4)}),
                "Measured request-relative timing; negative offsets precede request admission.",
            );
            tool.output_schema["properties"]["changes"] = open(
                json!({"hooks":strings(),"scripts":array(object(json!({"script":text(256),"phase":text(64),"contentAvailable":flag(),"annotationCount":count(),"diagnosticCount":count(),"annotations":array(object(json!({"note":text(2048),"tags":array(text(64),8),"severity":choice(&["info","warning","error"])}), &["note","tags","severity"]),16),"diagnostics":array(text(65_536),32)}), &["script","phase","contentAvailable","annotationCount","diagnosticCount"]),1024),"headers":array(json!({"type":"object","description":"Recorded hook ID, name, phase, changed flag and changed header names; no credential values."}),1024),"bodies":array(json!({"type":"object","description":"Recorded literal replacement outcomes with revision attribution; no payloads."}),1024),"autoResponse":nullable(json!({"type":"object","description":"Recorded winning local response and its immutable revision."}))}),
                "Captured change attribution independent of current saved rules.",
            );
            tool.output_schema["properties"]["transport"] = open(
                json!({"connections":array(json!({"type":"object","description":"Observed connection, protocol, TLS, setup and route evidence; no private TLS material."}),16),"routeSelection":maybe_text(),"routeAttempts":strings(),"diagnostics":strings(),"diagnosticsAvailable":flag(),"stream":nullable(json!({"type":"object","description":"Stream lifecycle, retention, loss and byte counters; no message payloads."})),"sequenceLoss":count()}),
                "Observed transport and stream context.",
            );
        }
        "transmog_rule_prepare"
        | "transmog_rule_update"
        | "transmog_rule_activate"
        | "transmog_rule_disable" => tool.output_schema = native_rule(),
        "transmog_rule_get" => {
            tool.output_schema = object(
                json!({"summary":rule_summary(),"rule":nullable(native_rule()),"definition":nullable(rule_definition()),"editable":flag(),"omitted":strings()}),
                &["summary", "rule", "definition", "editable", "omitted"],
            );
        }
        "transmog_rules_list" => {
            tool.output_schema = object(
                json!({"instanceId":instance(),"total":count(),"rules":array(rule_summary(),100),"nextOffset":nullable(count()),"coverage":text(1024)}),
                &["instanceId", "total", "rules", "nextOffset", "coverage"],
            );
        }
        "transmog_rule_test" => {
            let variants = rule_test();
            let mut fields = serde_json::Map::new();
            for variant in variants["anyOf"].as_array().expect("rule shapes") {
                fields.extend(
                    variant["properties"]
                        .as_object()
                        .expect("rule fields")
                        .clone(),
                );
            }
            fields.insert("kind".into(), choice(&["headers", "response-text"]));
            fields.insert("context".into(), super::context_ref());
            fields.insert("inlineComplete".into(), flag());
            fields.insert("source".into(), json!({"type":"object"}));
            fields.insert("ruleId".into(), id());
            fields.insert("revision".into(), count());
            fields.insert("beforeText".into(), text(262_144));
            fields.insert("afterText".into(), text(262_144));
            tool.output_schema = object(
                Value::Object(fields),
                &[
                    "kind",
                    "context",
                    "inlineComplete",
                    "source",
                    "ruleId",
                    "revision",
                ],
            );
        }
        "transmog_control_begin" | "transmog_control_renew" => {
            tool.output_schema = object(
                json!({"id":id(),"grantId":id(),"grantRevision":count(),"expiresAt":count(),"rules":array(id(),1024),"breakpoints":flag(),"controllerId":nullable(id())}),
                &[
                    "id",
                    "grantId",
                    "grantRevision",
                    "expiresAt",
                    "rules",
                    "breakpoints",
                    "controllerId",
                ],
            );
        }
        "transmog_control_end" => tool.output_schema = object(json!({"ended":flag()}), &["ended"]),
        "transmog_request_send" => {
            tool.output_schema = object(
                json!({"operationId":id(),"instanceId":instance(),"id":id(),"status":uint(599),"mode":choice(&["direct","live-rules"]),"attribution":text(128),"inspectWith":{"const":"transmog_exchange_get"}}),
                &[
                    "operationId",
                    "instanceId",
                    "id",
                    "status",
                    "attribution",
                    "inspectWith",
                ],
            );
        }
        "transmog_breakpoints_set" => {
            tool.output_schema = object(
                json!({"enabled":flag(),"controllerId":nullable(id())}),
                &["enabled", "controllerId"],
            );
        }
        "transmog_paused_list" => {
            tool.output_schema = object(
                json!({"controllerId":id(),"paused":array(open(json!({"decisionId":count(),"exchangeId":id(),"phase":choice(&["request-head","response-head","request-body","response-body"]),"requestHead":nullable(tool_head(false)),"responseHead":nullable(tool_head(true)),"bodyHex":maybe_text(),"hookId":text(128),"expiresAtUnixMs":count()}),"Single-use owned breakpoint decision envelope."),4),"totalPaused":count(),"nextOffset":nullable(count())}),
                &["controllerId", "paused", "totalPaused", "nextOffset"],
            );
        }
        "transmog_paused_decide" => {
            tool.output_schema = object(
                json!({"controllerId":id(),"pendingCount":count()}),
                &["controllerId", "pendingCount"],
            );
        }
        "transmog_exchange_compare" => {
            tool.output_schema = object(
                json!({"headers":open(json!({"state":choice(&["unavailable","different","no-visible-differences"]),"left":nullable(head()),"right":nullable(head())}),"Comparison of bounded retained heads."),"body":open(json!({"state":choice(&["not-compared","unavailable","incomplete","equal","different"]),"reason":text(1024),"left":{"type":"object"},"right":{"type":"object"}}),"Explicit bounded comparison coverage."),"coverage":text(1024)}),
                &["headers", "body", "coverage"],
            );
        }
        "transmog_stream_list" => {
            tool.output_schema = object(
                json!({"instanceId":instance(),"id":id(),"offset":count(),"total":count(),"nextOffset":nullable(count()),"rows":array(object(json!({"sequence":count(),"atMillis":count(),"sender":text(32),"kind":text(64),"observedBytes":count(),"truncated":flag(),"compressed":flag()}), &["sequence","atMillis","sender","kind","observedBytes","truncated","compressed"]),200)}),
                &["instanceId", "id", "offset", "total", "nextOffset", "rows"],
            );
        }
        "transmog_stream_read" => {
            tool.output_schema = object(
                json!({"instanceId":instance(),"id":id(),"sequence":count(),"encoding":choice(&["base64"]),"data":text(90_000),"offset":count(),"retainedBytes":count(),"observedBytes":count(),"truncated":flag(),"availability":choice(&["complete","truncated","unavailable"]),"nextOffset":nullable(count())}),
                &[
                    "instanceId",
                    "id",
                    "sequence",
                    "encoding",
                    "data",
                    "offset",
                    "retainedBytes",
                    "observedBytes",
                    "truncated",
                    "availability",
                    "nextOffset",
                ],
            );
        }
        _ => {}
    }
}

fn tool_head(response: bool) -> Value {
    let fields = array(
        object(
            json!({"name":text(256),"value":array(uint(255),131_072)}),
            &["name", "value"],
        ),
        256,
    );
    if response {
        object(
            json!({"status":uint(599),"headers":fields}),
            &["status", "headers"],
        )
    } else {
        object(
            json!({"method":text(32),"scheme":text(16),"authority":text(255),"path_and_query":text(16_384),"headers":fields}),
            &["method", "scheme", "authority", "path_and_query", "headers"],
        )
    }
}

fn rule_test() -> Value {
    let test = object(
        json!({"matched":flag(),"normalizedUrl":text(131_072),"checks":array(object(json!({"label":text(131_072),"matched":flag(),"detail":text(131_072)}), &["label","matched","detail"]),131_072),"captures":array(object(json!({"label":text(131_072),"value":text(131_072)}), &["label","value"]),131_072)}),
        &["matched", "normalizedUrl", "checks", "captures"],
    );
    let headers = array(
        object(
            json!({"name":text(131_072),"value":text(131_072),"encoding":choice(&["utf-8","base64"]),"sensitive":flag()}),
            &["name", "value", "encoding", "sensitive"],
        ),
        131_072,
    );
    json!({"type":"object","anyOf":[
        object(json!({"kind":{"const":"headers"},"test":test,"before":headers,"after":headers,"operations":array(object(json!({"index":count(),"operation":text(64),"name":text(131_072),"changed":flag()}), &["index","operation","name","changed"]),64)}), &["kind","test","before","after","operations"]),
        object(json!({"kind":{"const":"response-text"},"test":test,"report":nullable(open(json!({"outcome":text(64),"matches":nullable(count()),"replacements":count(),"beforeBytes":nullable(count()),"afterBytes":nullable(count()),"beforeSha256":maybe_text(),"afterSha256":maybe_text()}),"Network-free literal replacement outcome.")),"beforeText":text(65_536),"afterText":text(1_048_576)}), &["kind","test","report","beforeText","afterText"])
    ]})
}
