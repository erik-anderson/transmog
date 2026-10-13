//! Conventional XML timers are projections of local observations, never remote receipt.
use crate::{Session, xml_text};
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
fn xml(value: &str, projected: bool) -> String {
    let value = if projected {
        xml_text::project(value)
    } else {
        value.to_owned()
    };
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace(char::from(39), "&apos;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#9;")
}
pub(crate) fn timestamp(millis: u64) -> Option<String> {
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
    let mut flags = BTreeMap::new();
    let mut negotiation_flags = BTreeMap::new();
    if let Some(failure) = &session.evidence.failure {
        flags.insert("x-transmog-failure-category", failure.category.clone());
        flags.insert("x-transmog-failure-message", failure.message.clone());
    }
    if let Some(reason) = reason(session)
        && session
            .request
            .as_ref()
            .is_some_and(|request| request.method == "CONNECT")
    {
        flags.insert("x-transmog-original-connect-response-reason", reason.into());
    }
    if let Some(response) = session.response.as_ref()
        && let Some(exported) = crate::connections::opaque_response(session, response)
    {
        let names = exported
            .headers
            .iter()
            .skip(response.headers.len())
            .map(|field| String::from_utf8_lossy(&field.name))
            .collect::<Vec<_>>()
            .join(",");
        if !names.is_empty() {
            flags.insert("x-transmog-connect-export-headers", names);
        }
    }
    if session.kind == crate::SessionKind::ConnectionContext {
        flags.insert("x-transmog-connection-context", "true".into());
        flags.insert("ui-comments", "Transmog: generated connection summary; original CONNECT and ClientHello bytes were not captured.".into());
        flags.insert("x-responsebodytransferlength", "0".into());
    }
    let mut missing = Vec::new();
    if session.request.is_none() {
        flags.insert("x-transmog-missing-request-head", "true".into());
        missing.push("request");
    }
    if session.response.is_none() {
        flags.insert("x-transmog-missing-response-head", "true".into());
        if session.response_body.is_empty() {
            flags.insert("x-transmog-placeholder-response-body", "true".into());
        }
        missing.push("response");
    }
    if !missing.is_empty() {
        flags.insert(
            "ui-comments",
            format!(
                "Transmog placeholder: {} headers were not captured.",
                missing.join(" and ")
            ),
        );
    }
    if session.stream.is_some() || crate::websocket::established(session) {
        let note = if crate::websocket::established(session) {
            if let Some(request) = &session.request {
                flags.insert(
                    "x-transmog-websocket-original-method",
                    request.method.clone(),
                );
            }
            if let Some(response) = &session.response {
                flags.insert(
                    "x-transmog-websocket-original-status",
                    response.status.to_string(),
                );
            }
            flags.insert("x-transmog-websocket-framing", "Decoded messages reconstructed as uncompressed frames with archive-only zero client masks. Original fragmentation and compression are not retained. DoneRead is the local observation time; send times are unavailable.".into());
            flags.insert(
                "x-transmog-websocket-omitted",
                session.websocket_omitted.to_string(),
            );
            format!(
                "WebSocket messages are included. Framing and any HTTP/2 handshake are normalized for SAZ. {} incomplete or unavailable observations were omitted; save TMCap for full stream evidence.",
                session.websocket_omitted
            )
        } else if session.stream == Some(transmog_core::stream::StreamProtocol::Sse) {
            "Parsed SSE events are omitted; retained HTTP response bytes are included. Save TMCap for parsed stream activity.".into()
        } else {
            "The WebSocket handshake did not establish a captured connection.".into()
        };
        flags
            .entry("ui-comments")
            .and_modify(|value| {
                value.push(' ');
                value.push_str(&note);
            })
            .or_insert(note);
    }
    flags.insert(
        "x-transmog-terminal",
        match session.terminal {
            crate::TerminalState::Open => "active",
            crate::TerminalState::Completed => "completed",
            crate::TerminalState::Failed if crate::pending_at_save(session) => "active",
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
    if let Some(transmog_core::ClientIdentity::LocalProcess { pid, name }) =
        &session.evidence.client_identity
        && *pid != 0
    {
        flags.insert(
            "x-processinfo",
            format!("{}:{pid}", name.as_deref().unwrap_or_default()),
        );
    }
    for (leg, names) in [
        ("client", ["https-client-protocol", "https-client-cipher"]),
        ("upstream", ["https-server-protocol", "https-server-cipher"]),
    ] {
        if let Some(transport) = session
            .evidence
            .performance
            .transports
            .iter()
            .rev()
            .find(|transport| transport.leg == leg)
        {
            flags.insert(
                if leg == "client" {
                    "x-transmog-client-connection-id"
                } else {
                    "x-transmog-server-connection-id"
                },
                transport.connection_id.clone(),
            );
            flags.insert(
                if leg == "client" {
                    "x-transmog-client-connection-outcome"
                } else {
                    "x-transmog-server-connection-outcome"
                },
                transport.outcome.as_str().into(),
            );
            for (name, value) in names
                .into_iter()
                .zip([&transport.tls_version, &transport.cipher])
            {
                if let Some(value) = value {
                    flags.insert(name, value.clone());
                }
            }
            if let Some(prefix) = crate::tls::flag_prefix(leg) {
                if let Some(tls) = &transport.tls {
                    for (label, suffix) in crate::tls::NEGOTIATION_FIELDS {
                        if let Some(value) = tls.details.get(label) {
                            negotiation_flags.insert(format!("{prefix}{suffix}"), value.clone());
                        }
                    }
                }
                if let Some(resumed) = transport.tls_resumed {
                    negotiation_flags
                        .entry(format!("{prefix}session-resumed"))
                        .or_insert_with(|| if resumed { "Yes" } else { "No" }.into());
                }
            }
            if let Some(name) = transport
                .tls
                .as_ref()
                .and_then(|tls| tls.server_name.as_ref())
            {
                flags.insert(
                    if leg == "client" {
                        "https-client-sni"
                    } else {
                        "https-server-sni"
                    },
                    name.clone(),
                );
            }
            if leg == "client"
                && let Some(destination) = &transport.tunnel_destination
            {
                flags.insert("x-transmog-connect-destination", destination.clone());
            }
            if leg == "upstream"
                && let Some(peer) = transport
                    .peer
                    .as_ref()
                    .and_then(|peer| peer.parse::<std::net::SocketAddr>().ok())
            {
                flags.insert("x-hostIP", peer.ip().to_string());
            }
        }
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
    let projected = timers
        .values()
        .any(|value| xml_text::needs_projection(value))
        || flags
            .values()
            .any(|value| xml_text::needs_projection(value))
        || negotiation_flags
            .values()
            .any(|value| xml_text::needs_projection(value));
    if projected {
        flags.insert(xml_text::FLAG, xml_text::MODE.into());
        let note = "Transmog: XML-incompatible characters are shown as Unicode escapes in metadata. Transmog restores the original text on import.";
        flags
            .entry("ui-comments")
            .and_modify(|comments| {
                comments.push(' ');
                comments.push_str(note);
            })
            .or_insert_with(|| note.into());
    }
    let projection_attribute = if projected {
        format!(" {}=\"{}\"", xml_text::ATTRIBUTE, xml_text::MODE)
    } else {
        String::new()
    };
    let mut attributes = String::new();
    for (name, value) in &timers {
        let _ = write!(attributes, " {name}=\"{}\"", xml(value, projected));
    }
    let mut rendered_flags = String::new();
    for (name, value) in flags.iter().map(|(name, value)| (*name, value)).chain(
        negotiation_flags
            .iter()
            .map(|(name, value)| (name.as_str(), value)),
    ) {
        let _ = write!(
            rendered_flags,
            "<SessionFlag N=\"{name}\" V=\"{}\"/>",
            xml(value, projected)
        );
    }
    let websocket = crate::websocket::established(session);
    let connect = !websocket
        && session
            .request
            .as_ref()
            .is_some_and(|request| request.method == "CONNECT");
    let https = !connect
        && session
            .request
            .as_ref()
            .is_some_and(|request| request.target.starts_with("https://"));
    let bit_flags = u32::from(https)
        | 0x400
        | if websocket { 0x40000 } else { 0 }
        | if (session.kind == crate::SessionKind::ConnectionContext
            && session
                .response
                .as_ref()
                .is_some_and(|response| response.status == 200))
            || (connect
                && session.kind == crate::SessionKind::Captured
                && session
                    .evidence
                    .performance
                    .transports
                    .iter()
                    .any(|transport| {
                        transport.leg == "client"
                            && (transport.tls_version.is_some() || transport.tls.is_some())
                    }))
        {
            0x2000
        } else if connect
            && session
                .response
                .as_ref()
                .is_some_and(|response| response.status == 200)
            && crate::connections::opaque_transport(session).is_some()
        {
            0x1000
        } else {
            0
        };
    let mut tunnel_info = String::new();
    if let Some(transport) = crate::connections::opaque_transport(session) {
        for (name, count) in [
            ("BytesEgress", transport.bytes_written),
            ("BytesIngress", transport.bytes_read),
        ] {
            if let Some(count) = count {
                let _ = write!(tunnel_info, " {name}=\"{count}\"");
            }
        }
        tunnel_info = format!("<TunnelInfo{tunnel_info}/>");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><Session SID=\"{id}\" BitFlags=\"{bit_flags:x}\"{projection_attribute}><SessionTimers{attributes}/>{tunnel_info}<SessionFlags>{rendered_flags}</SessionFlags></Session>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_values_preserve_legal_characters_and_project_unrepresentable_ones() {
        let source = "&<>\"'\t\r\n \u{d7ff}\u{e000}\u{fffd}\u{10000}\u{10ffff}";
        let escaped = xml(source, false);
        assert_eq!(
            escaped,
            "&amp;&lt;&gt;&quot;&apos;&#9;&#13;&#10; \u{d7ff}\u{e000}\u{fffd}\u{10000}\u{10ffff}"
        );
        for character in (0..0x20)
            .filter_map(char::from_u32)
            .filter(|character| !matches!(character, '\t' | '\r' | '\n'))
            .chain(['\u{fffe}', '\u{ffff}'])
        {
            assert_eq!(
                xml(&character.to_string(), true),
                format!("\\u{{{:04x}}}", u32::from(character))
            );
        }
        assert_eq!(
            xml("literal\\u{0000}\0", true),
            "literal\\\\u{0000}\\u{0000}"
        );
        assert_eq!(xml("literal\\u{0000}", false), "literal\\u{0000}");
    }
}
