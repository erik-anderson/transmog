use super::*;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use transmog_core::observe::{ObserverEvent, ObserverEventKind};

struct TrafficHarness {
    harness: Harness,
    body: Vec<u8>,
}
impl std::ops::Deref for TrafficHarness {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}
impl TrafficHarness {
    fn grant(&self, permissions: &[Permission]) -> Grant {
        let mut grant = self.harness.grant(permissions);
        grant.scope = Scope::Hosts {
            hosts: vec!["127.0.0.1".into()],
        };
        grant
    }
}

fn traffic(body: Vec<u8>) -> TrafficHarness {
    let root = tempfile::tempdir().unwrap();
    let access = AccessManager::open(None).unwrap();
    access.set_enabled(true).unwrap();
    let application = Application::new(AppConfig {
        body_store: Some(transmog_app::BodyStoreConfig::product_default(
            root.path().join("bodies"),
        )),
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
    TrafficHarness {
        harness: Harness {
            _root: root,
            adapter,
            access,
        },
        body,
    }
}
fn fixture() -> TrafficHarness {
    traffic(
        br#"{"items":[{"id":"body-needle","active":true}],"nullValue":null,"a/b":{"~key":7}}"#
            .to_vec(),
    )
}
fn full(h: &TrafficHarness) -> Grant {
    h.grant(&[
        Permission::Inspect,
        Permission::Send,
        Permission::ReadBodies,
        Permission::ReadCredentials,
    ])
}
async fn send(h: &TrafficHarness, _: &Grant, name: &str) -> Value {
    if h.adapter.application.status().listener.is_none() {
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
    }
    let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = origin.local_addr().unwrap();
    let body = h.body.clone();
    let producer = tokio::spawn(async move {
        let (mut socket, _) = origin.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nSet-Cookie: credential-search-fixture\r\nX-Visible: visible-search-fixture\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
    });
    let mut socket =
        tokio::net::TcpStream::connect(h.adapter.application.status().listener.unwrap())
            .await
            .unwrap();
    socket.write_all(format!("GET http://{address}/{name} HTTP/1.1\r\nHost: {address}\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nAuthorization: Bearer supplied-fixture\r\nConnection: close, X-Hop\r\nX-Hop: drop\r\n\r\n").as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    producer.await.unwrap();
    let ids = h
        .adapter
        .application
        .matching_traffic_ids(&transmog_app::SessionQueryInput {
            search: Some(format!("/{name}")),
            ..Default::default()
        })
        .unwrap();
    let id = ids.last().unwrap();
    let exchange = transmog_core::intercept::ExchangeId(u128::from_str_radix(id, 16).unwrap());
    for _ in 0..200 {
        if h.adapter
            .application
            .session_service()
            .catalog()
            .get(exchange)
            .unwrap()
            .terminal
            .is_some()
        {
            return json!({"id":id});
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("loopback request did not reach a terminal observation")
}
fn contract(tool: &str, result: &Value) {
    let spec = crate::tool_catalog()
        .into_iter()
        .find(|spec| spec.name == tool)
        .unwrap();
    crate::catalog::validation::validate(&spec.output_schema, result)
        .unwrap_or_else(|error| panic!("{tool}: {error}"));
}

#[tokio::test]
async fn metadata_pages_are_uniform_frozen_and_owned_by_the_key_and_revision() {
    let h = fixture();
    let owner = full(&h);
    let one = send(&h, &owner, "one").await;
    let two = send(&h, &owner, "two").await;
    let first = h
        .call(
            &owner,
            "transmog_traffic_list",
            json!({"query":{"sort":"oldest","statusMin":200,"contentType":"json"},"limit":1}),
        )
        .await
        .unwrap();
    contract("transmog_traffic_list", &first);
    assert_eq!(first["sessions"][0]["id"], one["id"]);
    send(&h, &owner, "three").await;
    let second = h
        .call(
            &owner,
            "transmog_traffic_list",
            json!({"cursor":first["nextCursor"],"limit":1}),
        )
        .await
        .unwrap();
    assert_eq!(second["totalMatched"], 2);
    assert_eq!(second["sessions"][0]["id"], two["id"]);
    assert!(second["nextCursor"].is_null());
    let outsider = full(&h);
    assert_eq!(
        h.call(
            &outsider,
            "transmog_traffic_list",
            json!({"cursor":first["nextCursor"]})
        )
        .await
        .unwrap_err()
        .code,
        "expired_cursor"
    );
    let mut changed = owner.clone();
    changed.revision += 1;
    assert_eq!(
        h.call(
            &changed,
            "transmog_traffic_list",
            json!({"cursor":first["nextCursor"]})
        )
        .await
        .unwrap_err()
        .code,
        "expired_cursor"
    );
    let mut selected = owner.clone();
    selected.scope = Scope::Requests {
        instance_id: h.adapter.instance_id.clone(),
        ids: vec![one["id"].as_str().unwrap().into()],
    };
    let selected = h
        .call(&selected, "transmog_traffic_list", json!({}))
        .await
        .unwrap();
    assert_eq!(selected["sessions"][0], first["sessions"][0]);
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn search_applies_scope_and_disclosure_before_matching_or_counting() {
    let h = fixture();
    let owner = full(&h);
    send(&h, &owner, "one").await;
    let reader = h.grant(&[Permission::Inspect]);
    let visible = h
        .call(
            &reader,
            "transmog_traffic_search",
            json!({"pattern":"visible-search-fixture","fields":["response-headers"]}),
        )
        .await
        .unwrap();
    assert_eq!(visible["totalMatched"], 1);
    contract("transmog_traffic_search", &visible);
    let secret = h
        .call(
            &reader,
            "transmog_traffic_search",
            json!({"pattern":"credential-search-fixture","fields":["response-headers"]}),
        )
        .await
        .unwrap();
    assert_eq!(secret["totalMatched"], 0);
    let names = h
        .call(
            &reader,
            "transmog_traffic_search",
            json!({"pattern":"Set-Cookie","fields":["response-headers"]}),
        )
        .await
        .unwrap();
    assert_eq!(names["totalMatched"], 1);
    assert!(
        h.call(
            &reader,
            "transmog_traffic_search",
            json!({"pattern":"body-needle","fields":["response-bodies"]})
        )
        .await
        .is_err()
    );
    let body = h
        .call(
            &owner,
            "transmog_traffic_search",
            json!({"pattern":"body-needle","fields":["response-bodies"]}),
        )
        .await
        .unwrap();
    assert_eq!(body["totalMatched"], 1);
    assert_eq!(body["searchCoverage"]["examined"], 1);
    let mut outside = owner.clone();
    outside.scope = Scope::Hosts {
        hosts: vec!["outside.test".into()],
    };
    let result = h
        .call(
            &outside,
            "transmog_traffic_search",
            json!({"pattern":"body-needle","fields":["response-bodies"]}),
        )
        .await
        .unwrap();
    assert_eq!(result["searchCoverage"]["examined"], 0);
    assert_eq!(result["totalMatched"], 0);
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn search_cursors_recheck_the_specific_disclosure_used_to_find_matches() {
    let h = fixture();
    let owner = full(&h);
    send(&h, &owner, "one").await;
    send(&h, &owner, "two").await;
    let page = h
        .call(
            &owner,
            "transmog_traffic_search",
            json!({"pattern":"credential-search-fixture","fields":["response-headers"],"limit":1}),
        )
        .await
        .unwrap();
    assert_eq!(page["totalMatched"], 2);
    let mut restricted = owner.clone();
    restricted.permissions.remove(&Permission::ReadCredentials);
    assert!(
        h.call(
            &restricted,
            "transmog_traffic_list",
            json!({"cursor":page["nextCursor"]})
        )
        .await
        .is_err()
    );
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn compact_batch_and_json_extraction_preserve_coverage_and_json_null() {
    let h = fixture();
    let owner = full(&h);
    let sent = send(&h, &owner, "one").await;
    let batch = h
        .call(
            &owner,
            "transmog_exchanges_get",
            json!({"ids":[sent["id"],"00000000000000000000000000000000"],"include":["timing"]}),
        )
        .await
        .unwrap();
    contract("transmog_exchanges_get", &batch);
    assert!(batch["items"][0].get("requests").is_none());
    assert!(batch["items"][0].get("timing").is_some());
    assert!(batch["items"][1].get("error").is_some());
    assert!(!batch.to_string().contains("body-needle"));
    let probe = h
        .call(
            &owner,
            "transmog_body_read",
            json!({"id":sent["id"],"boundary":"client-response","decodeContent":true}),
        )
        .await
        .unwrap();
    assert_eq!(probe["metadata"]["availability"], "complete", "{probe}");
    assert_eq!(probe["representation"], "original-text", "{probe}");
    let result=h.call(&owner,"transmog_body_extract",json!({"id":sent["id"],"boundary":"client-response","pointers":["/items/0/id","/nullValue","/missing","/a~1b/~0key"]})).await.unwrap();
    contract("transmog_body_extract", &result);
    assert_eq!(result["values"][0]["value"], "body-needle");
    assert_eq!(result["values"][1]["found"], true);
    assert!(result["values"][1]["value"].is_null());
    assert_eq!(result["values"][2]["found"], false);
    assert!(result["values"][2].get("value").is_none());
    assert_eq!(result["values"][3]["value"], 7);
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn extraction_rejects_oversized_json_instead_of_parsing_a_prefix() {
    let h = traffic(serde_json::to_vec(&json!({"data":"x".repeat(70000)})).unwrap());
    let owner = full(&h);
    let sent = send(&h, &owner, "one").await;
    assert_eq!(
        h.call(
            &owner,
            "transmog_body_extract",
            json!({"id":sent["id"],"boundary":"client-response","pointers":["/data"]})
        )
        .await
        .unwrap_err()
        .code,
        "incomplete_body"
    );
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_preparation_is_read_only_and_drops_credentials_and_connection_headers() {
    let h = fixture();
    let owner = full(&h);
    let sent = send(&h, &owner, "one").await;
    let reader = h.grant(&[Permission::Inspect]);
    let prepared = h
        .call(
            &reader,
            "transmog_request_prepare",
            json!({"id":sent["id"]}),
        )
        .await
        .unwrap();
    contract("transmog_request_prepare", &prepared);
    let headers = prepared["request"]["headers"].as_array().unwrap();
    assert_eq!(
        headers
            .iter()
            .filter(|header| header["name"] == "X-Duplicate")
            .count(),
        2
    );
    assert!(!prepared.to_string().contains("supplied-fixture"));
    assert!(headers.iter().all(|header| {
        !header["name"]
            .as_str()
            .unwrap()
            .eq_ignore_ascii_case("X-Hop")
    }));
    assert_eq!(
        h.call(&owner, "transmog_traffic_list", json!({}))
            .await
            .unwrap()["totalMatched"],
        1
    );
    assert!(
        h.call(
            &reader,
            "transmog_request_prepare",
            json!({"id":sent["id"],"includeCredentials":true})
        )
        .await
        .is_err()
    );
    let allowed = h
        .call(
            &owner,
            "transmog_request_prepare",
            json!({"id":sent["id"],"includeCredentials":true}),
        )
        .await
        .unwrap();
    assert!(allowed.to_string().contains("supplied-fixture"));
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn waiting_does_not_block_other_tools_and_timeout_is_an_empty_success() {
    let h = fixture();
    let owner = full(&h);
    let input =
        json!({"instanceId":h.adapter.instance_id,"afterRequestNumber":"0","timeoutMs":1000});
    let wait = h
        .adapter
        .handle(owner.clone(), "transmog_traffic_wait", input);
    let send = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        send(&h, &owner, "wake").await
    };
    let (result, produced) = tokio::join!(wait, send);
    let result = result.unwrap();
    contract("transmog_traffic_wait", &result);
    assert_eq!(result["timedOut"], false);
    assert_eq!(result["sessions"][0]["id"], produced["id"]);
    let empty = h
        .call(
            &owner,
            "transmog_traffic_wait",
            json!({"afterRequestNumber":"18446744073709551615","timeoutMs":0}),
        )
        .await
        .unwrap();
    assert_eq!(empty["timedOut"], true);
    assert_eq!(empty["totalMatched"], 0);
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn stream_metadata_never_includes_payload_and_reads_are_bounded_and_protected() {
    let h = fixture();
    let owner = full(&h);
    let sent = send(&h, &owner, "stream").await;
    let id = transmog_core::intercept::ExchangeId(
        u128::from_str_radix(sent["id"].as_str().unwrap(), 16).unwrap(),
    );
    let catalog = h.adapter.application.session_service().catalog();
    catalog.set_stream_content_retention(true, true);
    let sequence = catalog.sequence(id).unwrap() + 1;
    let mut message = transmog_core::stream::StreamMessage::new(
        "server",
        "event",
        bytes::Bytes::from_static(b"stream-secret"),
    );
    message.raw = Some("data: stream-secret".into());
    message.event_id = Some("private-event-id".into());
    catalog.apply(ObserverEvent {
        exchange_id: id,
        sequence,
        kind: ObserverEventKind::StreamMessage(message),
    });
    let reader = h.grant(&[Permission::Inspect]);
    let list = h
        .call(&reader, "transmog_stream_list", json!({"id":sent["id"]}))
        .await
        .unwrap();
    contract("transmog_stream_list", &list);
    assert_eq!(list["total"], 1);
    assert!(!list.to_string().contains("stream-secret"));
    assert!(!list.to_string().contains("private-event-id"));
    assert!(
        h.call(
            &reader,
            "transmog_stream_read",
            json!({"id":sent["id"],"sequence":sequence})
        )
        .await
        .is_err()
    );
    let read = h
        .call(
            &owner,
            "transmog_stream_read",
            json!({"id":sent["id"],"sequence":sequence,"maxBytes":6}),
        )
        .await
        .unwrap();
    contract("transmog_stream_read", &read);
    assert_eq!(
        STANDARD.decode(read["data"].as_str().unwrap()).unwrap(),
        b"stream"
    );
    assert_eq!(read["nextOffset"], 6);
    assert_eq!(read["availability"], "complete");
    let end = h
        .call(
            &reader,
            "transmog_stream_list",
            json!({"id":sent["id"],"offset":1}),
        )
        .await
        .unwrap();
    assert_eq!(end["rows"], json!([]));
    assert!(end["nextOffset"].is_null());
    h.adapter.application.shutdown().await.unwrap();
}

#[test]
fn nested_validation_names_the_invalid_field_without_echoing_its_value() {
    let error=validate_arguments("transmog_request_send",&json!({"instanceId":"0".repeat(32),"operationId":"one","request":{"method":"GET","url":"http://example.test","headers":[{"name":"X-Test","value":123}]}})).unwrap_err();
    assert!(error.message.contains("arguments.request.headers[0].value"));
    assert!(!error.message.contains("123"));
    assert!(validate_arguments("transmog_request_send",&json!({"instanceId":"wrong","operationId":"one","request":{"method":"GET","url":"http://example.test"}})).is_err());
}

#[tokio::test]
async fn explicit_rule_definitions_test_and_update_without_losing_human_match_conditions() {
    let h = Harness::new();
    let owner = h.grant(&[
        Permission::PrepareChanges,
        Permission::EditSharedRules,
        Permission::ReadBodies,
    ]);
    let prepared=h.call(&owner,"transmog_rule_prepare",json!({"rule":{"kind":"headers","name":"Add debug header","host":"example.test","phase":"response","operations":[{"operation":"append","name":"X-Debug","value":"enabled"}]}})).await.unwrap();
    contract("transmog_rule_prepare", &prepared);
    let tested=h.call(&owner,"transmog_rule_test",json!({"id":prepared["id"],"sample":{"method":"GET","url":"http://example.test/","responseHeaders":[{"name":"X-Debug","value":"existing"}]}})).await.unwrap();
    assert!(tested.to_string().contains("enabled"));
    contract("transmog_rule_test", &tested);
    let mut native = rule("human-conditions", false);
    native.matcher.query = Some("version=2".into());
    let native: Rule = parse(h.adapter.save_rule(native, None).unwrap()).unwrap();
    let updated=h.call(&owner,"transmog_rule_update",json!({"id":native.id,"expectedRevision":native.revision,"rule":{"kind":"headers","name":"Collaborative edit","host":"example.test","phase":"request","operations":[{"operation":"set","name":"X-Debug","value":"revised"}]}})).await.unwrap();
    assert_eq!(updated["matcher"]["query"], "version=2");
    let read = h
        .call(&owner, "transmog_rule_get", json!({"id":updated["id"]}))
        .await
        .unwrap();
    contract("transmog_rule_get", &read);
    assert_eq!(read["definition"]["operations"][0]["value"], "revised");
    let text=h.call(&owner,"transmog_rule_prepare",json!({"rule":{"kind":"response-text","name":"Replace label","host":"example.test","find":"before","replacement":"after","mediaType":"text/plain"}})).await.unwrap();
    let tested=h.call(&owner,"transmog_rule_test",json!({"id":text["id"],"sample":{"method":"GET","url":"http://example.test/","responseHeaders":[{"name":"Content-Type","value":"text/plain; charset=utf-8"}]},"body":"before"})).await.unwrap();
    assert!(tested.to_string().contains("after"));
    contract("transmog_rule_test", &tested);
    h.adapter.application.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_explains_credentials_omitted_by_source_privacy() {
    let h = fixture();
    let mut state = h.adapter.application.product_state();
    state.privacy.redact_sensitive_headers = true;
    h.adapter.application.save_product_state(state).unwrap();
    let owner = full(&h);
    let captured = send(&h, &owner, "redacted-source").await;
    let draft = h
        .call(
            &owner,
            "transmog_request_prepare",
            json!({"id":captured["id"],"includeCredentials":true}),
        )
        .await
        .unwrap();
    assert!(!draft.to_string().contains("supplied-fixture"));
    assert!(
        draft["droppedHeaders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|header| header["name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case("authorization"))
                && header["reason"]
                    .as_str()
                    .unwrap()
                    .contains("source capture"))
    );
    h.adapter.application.shutdown().await.unwrap();
}
