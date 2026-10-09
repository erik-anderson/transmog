//! Interpret named `NetLog` constants rather than hard-coding Chromium event IDs.
use super::{
    AppError, AtomicBool, BTreeMap, BodySnapshot, ClientIdentity, Duration, ExchangeBoundary,
    ExchangeId, HashMap, HeaderBlock, HeaderField, ImportData, MAX_SESSIONS, Ordering,
    ResponseHead, SessionTerminal, SystemTime, TraceEntry, TransportObservation, Value, charge,
    empty, empty_snapshot, invalid, metadata, native_request, protocol, string, unavailable,
    version,
};
use super::{BodySpool, Engine, STANDARD};
use crate::traces::import_failure;
use std::collections::HashSet;

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}
fn name<'a>(constants: &'a Value, table: &str, value: &'a Value) -> &'a str {
    value
        .as_str()
        .filter(|s| s.parse::<u64>().is_err())
        .or_else(|| {
            constants[table]
                .as_object()?
                .iter()
                .find(|(_, id)| *id == value)
                .map(|(name, _)| name.as_str())
        })
        .unwrap_or_default()
}
type StatusLine = (u16, String, String);
fn fields(value: &Value) -> Result<(HeaderBlock, Option<StatusLine>), AppError> {
    let mut headers = HeaderBlock::default();
    let mut status = None;
    let lines = match value {
        Value::Array(lines) => lines
            .iter()
            .filter_map(Value::as_str)
            .flat_map(|s| s.lines())
            .collect::<Vec<_>>(),
        Value::String(text) => text.lines().collect(),
        _ => vec![],
    };
    for line in lines {
        if line.starts_with("HTTP/") {
            let mut parts = line.splitn(3, ' ');
            let protocol = parts.next().unwrap_or_default().into();
            if let Some(code) = parts.next().and_then(|s| s.parse().ok()) {
                status = Some((code, protocol, parts.next().unwrap_or_default().into()));
            }
        } else if let Some(rest) = line.strip_prefix(":status:") {
            if let Ok(code) = rest.trim().parse() {
                status = Some((code, "HTTP/2".into(), String::new()));
            }
        } else if !line.starts_with(':')
            && let Some((key, value)) = line.split_once(':')
        {
            headers.push(
                HeaderField::try_new(key.trim(), value.trim())
                    .map_err(|_| invalid("NetLog contains an invalid HTTP header"))?,
            );
        }
    }
    Ok((headers, status))
}
fn wall(ticks: f64, offset: Option<f64>) -> SystemTime {
    offset
        .and_then(|offset| Duration::try_from_secs_f64((ticks + offset) / 1000.0).ok())
        .and_then(|duration| SystemTime::UNIX_EPOCH.checked_add(duration))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

#[allow(clippy::too_many_lines)]
pub(super) fn import(
    document: &Value,
    prefix: u64,
    trace_id: &str,
    canceled: &AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> Result<ImportData, AppError> {
    let constants = &document["constants"];
    if !constants.is_object() {
        return Err(invalid("NetLog constants are missing"));
    }
    let events = document["events"]
        .as_array()
        .ok_or_else(|| invalid("NetLog events must be an array"))?;
    let mut sources: BTreeMap<u64, Vec<&Value>> = BTreeMap::new();
    for event in events {
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Trace import canceled"));
        }
        if let Some(id) = event["source"]["id"].as_u64() {
            sources.entry(id).or_default().push(event);
        }
    }
    for group in sources.values_mut() {
        group.sort_by(|a, b| {
            number(&a["time"])
                .partial_cmp(&number(&b["time"]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    let mut result = empty(
        "netlog",
        serde_json::json!({"constants":constants,"polledData":document["polledData"],"userComments":document["userComments"]}),
    );
    let offset = number(&constants["timeTickOffset"]);
    let mut position = 0;
    let mut spool = BodySpool::new()?;
    for (source_id, group) in &sources {
        if !group.iter().any(|e| {
            matches!(
                name(constants, "logSourceType", &e["source"]["type"]),
                "URL_REQUEST" | "DOH_URL_REQUEST"
            )
        }) {
            continue;
        }
        // A URL_REQUEST source can contain redirect hops. Each start-job owns
        // the following heads until the next start-job; never mix their bodies.
        let starts = group
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let event = name(constants, "logEventTypes", &e["type"]);
                event == "URL_REQUEST_START_JOB"
                    && e["params"]["url"].is_string()
                    && name(constants, "logEventPhase", &e["phase"]) != "PHASE_END"
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let starts = if starts.is_empty() {
            group
                .iter()
                .position(|e| e["params"]["url"].is_string())
                .into_iter()
                .collect()
        } else {
            starts
        };
        for (hop, begin) in starts.iter().enumerate() {
            if canceled.load(Ordering::Acquire) {
                return Err(unavailable("Trace import canceled"));
            }
            if position >= MAX_SESSIONS {
                return Err(invalid("NetLog exceeds the viewer session limit"));
            }
            let end = starts.get(hop + 1).copied().unwrap_or(group.len());
            let slice = &group[*begin..end];
            let params = &group[*begin]["params"];
            let method = params["method"].as_str().unwrap_or("GET");
            let Ok(mut request) = native_request(method, string(params, "url"), vec![]) else {
                if result.issues.len() < 512 {
                    result
                        .issues
                        .push(format!("Source {source_id}: unsupported request URL"));
                }
                continue;
            };
            position += 1;
            let id = ExchangeId(u128::from(prefix) << 64 | position as u128);
            let start_ticks = number(&group[*begin]["time"]).unwrap_or(0.0);
            let last_ticks = slice
                .last()
                .and_then(|e| number(&e["time"]))
                .unwrap_or(start_ticks);
            let mut response = None;
            let mut request_protocol = String::new();
            let mut response_protocol = String::new();
            let mut reason = String::new();
            let mut timings = BTreeMap::new();
            timings.insert("NetLog source".into(), source_id.to_string());
            timings.insert(
                "NetLog start (monotonic ms)".into(),
                start_ticks.to_string(),
            );
            timings.insert(
                "NetLog elapsed (ms)".into(),
                (last_ticks - start_ticks).to_string(),
            );
            if offset.is_none() {
                timings.insert(
                    "NetLog wall clock".into(),
                    "Unavailable: timeTickOffset was not recorded".into(),
                );
            }
            let mut phase_starts: HashMap<String, f64> = HashMap::new();
            let mut dependencies = Vec::new();
            let mut failed = None;
            let mut request_bytes = 0;
            let mut response_bytes = 0;
            let mut decoded_bytes = Vec::new();
            let mut body_recorded = true;
            let mut filtered_seen = false;
            let mut request_finished = false;
            for event in slice {
                let event_name = name(constants, "logEventTypes", &event["type"]);
                let params = &event["params"];
                if event_name == "REQUEST_ALIVE"
                    && name(constants, "logEventPhase", &event["phase"]) == "PHASE_END"
                {
                    request_finished = true;
                }
                if event_name == "URL_REQUEST_JOB_FILTERED_BYTES_READ" {
                    filtered_seen = true;
                    if let Some(data) = params["bytes"]
                        .as_str()
                        .and_then(|text| STANDARD.decode(text).ok())
                        .filter(|bytes| Some(bytes.len() as u64) == params["byte_count"].as_u64())
                    {
                        decoded_bytes.extend_from_slice(&data);
                    } else {
                        body_recorded = false;
                    }
                }
                if let Some(dependency) = params["source_dependency"]["id"].as_u64() {
                    dependencies.push(dependency);
                }
                if event_name.contains("SEND_REQUEST_HEADERS") {
                    let (head, _) = fields(&params["headers"])?;
                    request.headers = head;
                    request_protocol = if event_name.contains("HTTP2") {
                        "HTTP/2".into()
                    } else if event_name.contains("QUIC") || event_name.contains("HTTP3") {
                        "HTTP/3".into()
                    } else {
                        string(params, "line")
                            .split_whitespace()
                            .last()
                            .unwrap_or_default()
                            .into()
                    };
                    request.source_version = version(&request_protocol);
                }
                if event_name.contains("READ_RESPONSE_HEADERS")
                    || event_name == "HTTP_TRANSACTION_READ_EARLY_HINTS_RESPONSE_HEADERS"
                {
                    let (head, status) = fields(&params["headers"])?;
                    if let Some((status, proto, text)) = status {
                        response_protocol = proto;
                        reason = text;
                        response = Some(ResponseHead {
                            status,
                            headers: head,
                            source_version: version(&response_protocol),
                        });
                    }
                }
                if let Some(error) = params["net_error"].as_i64().filter(|n| *n < -1)
                    && (event_name.starts_with("URL_REQUEST_JOB_")
                        || matches!(event_name, "URL_REQUEST_START_JOB" | "REQUEST_ALIVE"))
                {
                    failed = Some(error);
                }
                if event_name == "URL_REQUEST_JOB_BYTES_READ" {
                    response_bytes += params["byte_count"].as_u64().unwrap_or(0);
                }
                if event_name == "UPLOAD_DATA_STREAM_READ" {
                    request_bytes += params["byte_count"].as_u64().unwrap_or(0);
                }
                let phase = name(constants, "logEventPhase", &event["phase"]);
                if let Some(ticks) = number(&event["time"]) {
                    if phase == "PHASE_BEGIN" {
                        phase_starts.insert(event_name.into(), ticks);
                    } else if phase == "PHASE_END"
                        && let Some(begin) = phase_starts.remove(event_name)
                        && timings.len() < 128
                    {
                        timings.insert(
                            format!("NetLog {event_name} (ms)"),
                            (ticks - begin).to_string(),
                        );
                    }
                }
            }
            let started = wall(start_ticks, offset);
            let mut snapshot = empty_snapshot(
                metadata(
                    id,
                    request.target.clone(),
                    "127.0.0.1:0".parse().expect("constant endpoint"),
                    ClientIdentity::default(),
                    request.source_version,
                    started,
                ),
                Some(request.clone()),
                response.clone(),
                offset.map(|_| wall(last_ticks, offset)),
            );
            if !request_finished && failed.is_none() {
                snapshot.terminal = Some(import_failure(snapshot.metadata.clone()));
                timings.insert(
                    "NetLog completion".into(),
                    "Request end was not recorded".into(),
                );
            }
            protocol(
                &mut snapshot,
                ExchangeBoundary::ClientRequest,
                &request_protocol,
                None,
            );
            protocol(
                &mut snapshot,
                ExchangeBoundary::ClientResponse,
                &response_protocol,
                Some(reason),
            );
            // Dependencies expose physical setup. Follow directed links only,
            // with a fixed traversal bound; shared sockets never supply heads.
            let mut visited = HashSet::from([*source_id]);
            let mut transport = TransportObservation {
                leg: "upstream".into(),
                ..TransportObservation::default()
            };
            while let Some(dependency) = dependencies.pop() {
                if visited.len() >= 128 || !visited.insert(dependency) {
                    continue;
                }
                if let Some(group) = sources.get(&dependency) {
                    for event in group {
                        let params = &event["params"];
                        if let Some(id) = params["source_dependency"]["id"].as_u64() {
                            dependencies.push(id);
                        }
                        for (key, target) in [
                            ("remote_address", &mut transport.peer),
                            ("local_address", &mut transport.local),
                            ("version", &mut transport.tls_version),
                            ("cipher_suite", &mut transport.cipher),
                            ("negotiated_protocol", &mut transport.alpn),
                        ] {
                            if let Some(value) = params.get(key).filter(|v| !v.is_null()) {
                                *target = Some(
                                    value
                                        .as_str()
                                        .map_or_else(|| value.to_string(), str::to_owned),
                                );
                            }
                        }
                        let source_type =
                            name(constants, "logSourceType", &event["source"]["type"]);
                        if matches!(source_type, "SOCKET" | "HTTP2_SESSION" | "QUIC_SESSION") {
                            transport.connection_id = dependency.to_string();
                        }
                        let event_name = name(constants, "logEventTypes", &event["type"]);
                        if matches!(
                            event_name,
                            "TCP_CONNECT"
                                | "SSL_CONNECT"
                                | "HOST_RESOLVER_MANAGER_REQUEST"
                                | "HOST_RESOLVER_IMPL_REQUEST"
                        ) {
                            let phase = name(constants, "logEventPhase", &event["phase"]);
                            if let Some(ticks) = number(&event["time"]) {
                                let key = format!("{dependency}:{event_name}");
                                if phase == "PHASE_BEGIN" {
                                    phase_starts.insert(key, ticks);
                                } else if phase == "PHASE_END"
                                    && let Some(begin) = phase_starts.remove(&key)
                                {
                                    timings.insert(
                                        format!("NetLog connection {dependency} {event_name} (ms)"),
                                        (ticks - begin).to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            if !transport.connection_id.is_empty() || transport.peer.is_some() {
                snapshot.performance.transports.push(transport);
            }
            if let Some(error) = failed {
                timings.insert("NetLog network error".into(), error.to_string());
                snapshot.terminal = Some(import_failure(snapshot.metadata.clone()));
                if let Some(SessionTerminal::Failed(failure)) = &mut snapshot.terminal {
                    failure.message = format!("Chromium network error {error}");
                }
            }
            snapshot.bodies.push(BodySnapshot {
                boundary: ExchangeBoundary::ClientRequest,
                observed_bytes: request_bytes,
                retained_prefix: bytes::Bytes::new(),
                truncated: true,
                trailers: None,
            });
            let bodyless = request.method == "HEAD"
                || response
                    .as_ref()
                    .is_some_and(|head| matches!(head.status, 204 | 304));
            let complete = (bodyless || filtered_seen && body_recorded)
                && request_finished
                && failed.is_none();
            let mut storage_response = response.clone();
            let mut body_headers = response
                .as_ref()
                .map(|head| head.headers.clone())
                .unwrap_or_default();
            if complete {
                body_headers.remove_all("content-encoding");
                body_headers.remove_all("transfer-encoding");
                body_headers.remove_all("content-length");
                body_headers.push(
                    HeaderField::try_new("Content-Length", decoded_bytes.len().to_string())
                        .expect("decimal length"),
                );
                if let Some(head) = &mut storage_response {
                    head.headers = body_headers.clone();
                }
            }
            if response_bytes == 0 {
                response_bytes = decoded_bytes.len() as u64;
            }
            spool.add(
                &mut result,
                &mut snapshot,
                ExchangeBoundary::ClientResponse,
                body_headers,
                storage_response,
                complete.then_some(decoded_bytes),
                response_bytes,
            )?;
            charge(
                &mut result,
                &serde_json::json!({"request":request.headers,"response":response.as_ref().map(|r| &r.headers),"timings":timings}),
            )?;
            result.entries.insert(format!("{:032x}", id.0), TraceEntry { trace_id: trace_id.into(), original_id: format!("{source_id}:{}", hop + 1), raw_headers: None, timings, protocol_known: !request_protocol.is_empty(), target_known: true, diagnostics: vec![if complete { "Response body recovered from Chromium's complete filtered-byte events. Request body bytes are unavailable." } else { "Complete HTTP body bytes are unavailable; socket bytes are not reconstructed as HTTP bodies." }.into()] });
            result.sessions.push(snapshot);
            progress(position as u64, sources.len() as u64);
        }
    }
    spool.finish(&mut result);
    result.notes.push("Chromium request heads, redirect hops, errors, connection context and original NetLog timings are mapped to Traffic. NetLog clocks remain labeled as imported evidence; missing bodies and measurements are not invented.".into());
    Ok(result)
}
