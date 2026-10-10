//! Conventional XML timers are projections of local observations, never remote receipt.
use crate::Session;
use std::{collections::BTreeMap, fmt::Write};
use transmog_core::performance::Milestone;

pub(crate) fn protocol(session: &Session, boundary: &str) -> String {
    let original = session
        .evidence
        .performance
        .protocols
        .iter()
        .find(|item| item.boundary == boundary)
        .map(|item| item.version.as_str());
    // SAZ is textual HTTP, so HTTP/2 and HTTP/3 are normalized to HTTP/1.1.
    // Their actual versions remain in flags and the extended evidence.
    original
        .filter(|version| matches!(*version, "HTTP/1.0" | "HTTP/1.1"))
        .unwrap_or("HTTP/1.1")
        .into()
}
pub(crate) fn reason(session: &Session) -> Option<&str> {
    session
        .evidence
        .performance
        .protocols
        .iter()
        .find(|item| item.boundary == "client-response")
        .and_then(|item| item.reason.as_deref())
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace(char::from(39), "&apos;")
}
fn timestamp(millis: u64) -> Option<String> {
    let nanos = i128::from(millis) * 1_000_000;
    let value = time::OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.millisecond()
    ))
}
#[allow(clippy::too_many_lines)] // One explicit, reviewable timer vocabulary projection.
pub(crate) fn render(
    session: &Session,
    id: usize,
    request_incomplete: bool,
    response_incomplete: bool,
) -> String {
    let mut timers = BTreeMap::new();
    if let Some(source) = session
        .evidence
        .provenance
        .as_ref()
        .and_then(|entry| entry.get("source"))
        && let Some(original) = source.get("timings").and_then(serde_json::Value::as_object)
    {
        for (name, value) in original {
            if !name.is_empty()
                && name.as_bytes()[0].is_ascii_alphabetic()
                && name.len() <= 256
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                && let Some(value) = value.as_str()
            {
                timers.insert(name.clone(), value.into());
            }
        }
    }
    for (milestone, name) in [
        (Milestone::ClientConnected, "ClientConnected"),
        (Milestone::RequestHeaders, "ClientBeginRequest"),
        (Milestone::RequestHeaders, "GotRequestHeaders"),
        (Milestone::ClientRequestDone, "ClientDoneRequest"),
        (Milestone::UpstreamConnected, "ServerConnected"),
        (Milestone::UpstreamBegin, "FiddlerBeginRequest"),
        (Milestone::ResponseHeaders, "GotResponseHeaders"),
        (Milestone::UpstreamResponseDone, "ServerDoneResponse"),
        (Milestone::ClientResponseBegin, "ClientBeginResponse"),
        (Milestone::ExchangeDone, "ClientDoneResponse"),
    ] {
        if let Some(point) = session
            .evidence
            .performance
            .points
            .iter()
            .find(|point| point.milestone == milestone)
            && let Some(value) = timestamp(point.unix_millis)
        {
            timers.entry(name.into()).or_insert(value);
        }
    }
    if let Some(started) = session
        .started_unix_nanos
        .and_then(|nanos| u64::try_from(nanos / 1_000_000).ok())
        .and_then(timestamp)
    {
        timers.entry("ClientBeginRequest".into()).or_insert(started);
    }
    if let Some(transport) = session
        .evidence
        .performance
        .transports
        .iter()
        .rev()
        .find(|item| item.leg == "upstream")
    {
        for (name, value) in [
            ("DNSTime", transport.dns_micros),
            ("TCPConnectTime", transport.tcp_micros),
            ("HTTPSHandshakeTime", transport.tls_micros),
        ] {
            if let Some(value) = value {
                timers
                    .entry(name.into())
                    .or_insert_with(|| (value / 1000).to_string());
            }
        }
    }
    // Classic's LoadMetadata passes these six attributes directly to
    // XmlConvert.ToDateTime, even when no timing was measured. Match Fiddler's
    // serialization of an unset DateTime instead of omitting a required field
    // or borrowing a timestamp from a different observation.
    for name in [
        "ClientConnected",
        "ClientDoneRequest",
        "ServerGotRequest",
        "ServerDoneResponse",
        "ClientBeginResponse",
        "ClientDoneResponse",
    ] {
        timers
            .entry(name.into())
            .or_insert_with(|| "0001-01-01T00:00:00".into());
    }
    let mut attributes = String::new();
    for (name, value) in &timers {
        let _ = write!(attributes, " {name}=\"{}\"", xml(value));
    }
    let mut flags = BTreeMap::new();
    flags.insert(
        "x-transmog-terminal",
        match session.terminal {
            crate::TerminalState::Open => "active",
            crate::TerminalState::Completed => "completed",
            crate::TerminalState::Failed => "failed",
        }
        .into(),
    );
    flags.insert("x-transmog-timer-semantics", "Local proxy observations; ClientBeginRequest means complete headers and ClientDoneResponse means processing ended, not peer receipt. Unmeasured required timers use DateTime.MinValue (0001-01-01T00:00:00).".into());
    if let Some(addr) = session
        .evidence
        .client_addr
        .as_ref()
        .and_then(|addr| addr.parse::<std::net::SocketAddr>().ok())
    {
        flags.insert("x-clientIP", addr.ip().to_string());
        flags.insert("x-clientport", addr.port().to_string());
    }
    for (boundary, name) in [
        ("client-request", "x-transmog-original-request-protocol"),
        ("client-response", "x-transmog-original-response-protocol"),
    ] {
        flags.insert(
            name,
            session
                .evidence
                .performance
                .protocols
                .iter()
                .find(|item| item.boundary == boundary)
                .map_or_else(|| "unavailable".into(), |item| item.version.clone()),
        );
    }
    if request_incomplete {
        flags.insert("log-drop-request-body", "true".into());
    }
    if response_incomplete {
        flags.insert("log-drop-response-body", "true".into());
    }
    let mut rendered_flags = String::new();
    for (name, value) in &flags {
        let _ = write!(
            rendered_flags,
            "<SessionFlag N=\"{name}\" V=\"{}\"/>",
            xml(value)
        );
    }
    let https = session
        .request
        .as_ref()
        .is_some_and(|request| request.target.starts_with("https://"));
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><Session SID=\"{id}\" BitFlags=\"{}\"><SessionTimers{attributes}/><SessionFlags>{rendered_flags}</SessionFlags></Session>",
        u8::from(https)
    )
}
