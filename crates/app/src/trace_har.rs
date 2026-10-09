//! HAR 1.2 output, streamed entry by entry through an atomic temporary file.
use crate::{AppError, Application, BodyStore, ErrorCategory};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::SystemTime,
};
use transmog_core::{HeaderBlock, observe::ExchangeBoundary};
use transmog_session::SessionSnapshot;

fn error(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, true)
}
fn date(time: SystemTime) -> String {
    let nanos = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(0),
        Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(0),
    };
    time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .ok()
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}
fn headers(block: &HeaderBlock, redact: bool) -> Value {
    Value::Array(block.iter().map(|field| {
        let private = ["authorization", "proxy-authorization", "cookie", "set-cookie"].iter().any(|name| field.name_eq(name));
        json!({"name":String::from_utf8_lossy(field.name()),"value":if field.is_redacted() || redact && private { "[redacted]".into() } else { String::from_utf8_lossy(field.value()).into_owned() }})
    }).collect())
}
fn header(block: &HeaderBlock, name: &str) -> String {
    block
        .values(name)
        .next()
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .unwrap_or_default()
}
fn bytes(
    store: &BodyStore,
    snapshot: &SessionSnapshot,
    boundary: ExchangeBoundary,
) -> Option<Vec<u8>> {
    let mut body = store.open_complete(snapshot.exchange_id, boundary).ok()?;
    let mut bytes = Vec::new();
    body.read_to_end(&mut bytes).ok()?;
    if body.metadata().length_known && bytes.len() as u64 != body.metadata().retained_bytes {
        return None;
    }
    Some(bytes)
}

pub(crate) fn write(
    application: &Application,
    destination: PathBuf,
    redact: bool,
) -> Result<crate::TraceSaveResult, AppError> {
    if !destination.is_absolute() {
        return Err(error("Choose an absolute HAR destination"));
    }
    let store = application
        .body_store()
        .ok_or_else(|| error("Body retention is not configured"))?;
    store
        .flush()
        .map_err(|_| error("Body metadata is unavailable"))?;
    let mut sessions = application
        .session_service()
        .catalog()
        .project_retained(Clone::clone);
    sessions.sort_by_key(|session| session.metadata.started_at);
    let mut temporary = tempfile::NamedTempFile::new_in(
        destination
            .parent()
            .ok_or_else(|| error("Choose a HAR destination"))?,
    )
    .map_err(|_| error("HAR output could not be created"))?;
    temporary
        .write_all(b"{\"log\":{\"version\":\"1.2\",\"creator\":")
        .map_err(|_| error("HAR output could not be written"))?;
    serde_json::to_writer(
        &mut temporary,
        &json!({"name":"Transmog","version":env!("CARGO_PKG_VERSION")}),
    )
    .map_err(|_| error("HAR output could not be written"))?;
    temporary
        .write_all(b",\"entries\":[")
        .map_err(|_| error("HAR output could not be written"))?;
    let runtime = tokio::runtime::Handle::current();
    let mut incomplete = 0;
    for (position, snapshot) in sessions.iter().enumerate() {
        let entry = entry(
            application,
            store,
            snapshot,
            redact,
            &runtime,
            &mut incomplete,
        )?;
        if position != 0 {
            temporary
                .write_all(b",")
                .map_err(|_| error("HAR output failed"))?;
        }
        serde_json::to_writer(&mut temporary, &entry).map_err(|_| error("HAR output failed"))?;
    }
    temporary
        .write_all(b"]}}")
        .map_err(|_| error("HAR output failed"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| error("HAR output could not be flushed"))?;
    let size = temporary
        .as_file()
        .metadata()
        .map_err(|_| error("HAR size is unavailable"))?
        .len();
    temporary
        .persist(&destination)
        .map_err(|_| error("HAR could not be saved at this destination"))?;
    Ok(crate::TraceSaveResult {
        destination,
        entries: sessions.len(),
        bytes: size,
        incomplete_bodies: incomplete,
    })
}

#[allow(clippy::too_many_lines)]
fn entry(
    application: &Application,
    store: &BodyStore,
    snapshot: &SessionSnapshot,
    redact: bool,
    runtime: &tokio::runtime::Handle,
    incomplete: &mut usize,
) -> Result<Value, AppError> {
    let request = snapshot
        .request_heads
        .iter()
        .find(|head| head.boundary == ExchangeBoundary::ClientRequest)
        .or_else(|| snapshot.request_heads.first());
    let response = snapshot
        .response_heads
        .iter()
        .find(|head| head.boundary == ExchangeBoundary::ClientResponse)
        .or_else(|| snapshot.response_heads.last());
    let empty = HeaderBlock::default();
    let req_headers = request.map_or(&empty, |r| &r.head.headers);
    let resp_headers = response.map_or(&empty, |r| &r.head.headers);
    let target = request.map_or(snapshot.metadata.original_target.as_target(), |r| {
        &r.head.target
    });
    let url = format!(
        "{}://{}{}{}",
        target.scheme,
        target.authority,
        target.path,
        target
            .query
            .as_ref()
            .map_or(String::new(), |q| format!("?{q}"))
    );
    let protocol = |boundary: &str| {
        snapshot
            .performance
            .protocols
            .iter()
            .find(|p| p.boundary == boundary)
            .map_or("", |p| p.version.as_str())
    };
    let request_body = bytes(store, snapshot, ExchangeBoundary::ClientRequest);
    let size = |boundary| {
        snapshot
            .bodies
            .iter()
            .find(|b| b.boundary == boundary)
            .map(|b| b.observed_bytes)
    };
    let req_size = size(ExchangeBoundary::ClientRequest)
        .or_else(|| request_body.as_ref().map(|b| b.len() as u64));
    let query = url::Url::parse(&url)
        .ok()
        .map(|url| {
            url.query_pairs()
                .map(|(name, value)| json!({"name":name,"value":value}))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut req = json!({"method":request.map_or("GET", |r| r.head.method.as_str()),"url":url,"httpVersion":protocol("client-request"),"cookies":[],"headers":headers(req_headers,redact),"queryString":query,"headersSize":-1,"bodySize":req_size.map_or(-1_i128, i128::from)});
    if let Some(body) = request_body {
        if !body.is_empty() {
            if let Ok(text) = std::str::from_utf8(&body) {
                req["postData"] =
                    json!({"mimeType":header(req_headers,"content-type"),"text":text});
            } else {
                req["_transmogBodyBase64"] = json!(STANDARD.encode(body));
                req["comment"] = json!(
                    "Binary request body is preserved in a Transmog extension; HAR postData.text cannot represent these bytes."
                );
            }
        }
    } else {
        *incomplete += 1;
        req["comment"] = json!("Complete request body unavailable.");
    }
    let mut content = json!({"size":0,"mimeType":header(resp_headers,"content-type")});
    let id = format!("{:032x}", snapshot.exchange_id.0);
    if let Ok(file) = application.prepare_response_file(&id, "client-response") {
        let decoded = tempfile::tempdir().map_err(|_| error("HAR decoding cache unavailable"))?;
        let decoded_path = decoded.path().join("body");
        if let Ok(result) = runtime.block_on(file.write_to(&decoded_path, u64::MAX)) {
            let body = std::fs::read(&decoded_path)
                .map_err(|_| error("Decoded HAR body is unreadable"))?;
            content["size"] = json!(result.bytes);
            content["text"] = json!(STANDARD.encode(body));
            content["encoding"] = json!("base64");
            if let Some(wire) = size(ExchangeBoundary::ClientResponse) {
                content["compression"] = json!(i128::from(result.bytes) - i128::from(wire));
            }
        } else {
            *incomplete += 1;
            content["comment"] = json!("Complete decoded response body unavailable.");
            content["size"] = json!(size(ExchangeBoundary::ClientResponse).unwrap_or(0));
        }
    } else {
        *incomplete += 1;
        content["comment"] = json!("Complete response body unavailable.");
        content["size"] = json!(size(ExchangeBoundary::ClientResponse).unwrap_or(0));
    }
    let duration = snapshot
        .terminal_at
        .and_then(|end| end.duration_since(snapshot.metadata.started_at).ok())
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    // Required HAR phases are nonnegative. Unmeasured breakdown is disclosed;
    // original imported timings override this elapsed-only representation.
    let mut timings = json!({"send":0,"wait":duration,"receive":0,"blocked":-1,"dns":-1,"connect":-1,"ssl":-1,"comment":"Elapsed exchange time only; phase breakdown was not measured."});
    if let Some(source) = application.traces.entry(&id) {
        for key in [
            "send", "wait", "receive", "blocked", "dns", "connect", "ssl",
        ] {
            if let Some(value) = source
                .timings
                .get(&format!("HAR {key} (ms)"))
                .and_then(|s| s.parse::<f64>().ok())
            {
                timings[key] = json!(value);
            }
        }
    }
    let duration = ["send", "wait", "receive", "blocked", "dns", "connect"]
        .iter()
        .filter_map(|key| timings[*key].as_f64())
        .filter(|n| *n >= 0.0)
        .sum::<f64>();
    Ok(
        json!({"startedDateTime":date(snapshot.metadata.started_at),"time":duration,"request":req,"response":{"status":response.map_or(0,|r|r.head.status),"statusText":snapshot.performance.protocols.iter().find(|p|p.boundary=="client-response").and_then(|p|p.reason.as_deref()).unwrap_or(""),"httpVersion":protocol("client-response"),"cookies":[],"headers":headers(resp_headers,redact),"content":content,"redirectURL":header(resp_headers,"location"),"headersSize":-1,"bodySize":size(ExchangeBoundary::ClientResponse).map_or(-1_i128,i128::from)},"cache":{},"timings":timings}),
    )
}
