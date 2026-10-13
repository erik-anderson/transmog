//! Labeled connection summaries projected from captured physical transport facts.
use crate::{BodyCompletion, CapturedRequest, CapturedResponse, Session, TerminalState};
use std::{
    collections::{BTreeMap, HashSet},
    fmt::Write,
};
use transmog_core::performance::{Milestone, TransportObservation, TransportOutcome};

struct Summary {
    session: Session,
    started: Option<i128>,
    observed: Option<i128>,
    setup_ended: Option<i128>,
}

pub(crate) fn summaries(sessions: &BTreeMap<u128, Session>) -> Vec<Session> {
    let captured = captured_connects(sessions);
    let mut summaries = BTreeMap::<(String, String), Summary>::new();
    for (id, session) in sessions {
        let Some(request) = &session.request else {
            continue;
        };
        let Ok(target) = request.target.parse::<http::Uri>() else {
            continue;
        };
        if request.method == "CONNECT" || target.scheme_str() != Some("https") {
            continue;
        }
        let namespace = namespace(session);
        for client in session
            .evidence
            .performance
            .transports
            .iter()
            .filter(|transport| {
                transport.leg == "client"
                    && !transport.connection_id.is_empty()
                    && (transport.tls_version.is_some() || transport.tls.is_some())
            })
        {
            let Some(authority) = destination(client, &target) else {
                continue;
            };
            if session.evidence.client_addr.as_ref().is_some_and(|addr| {
                captured.contains(&(
                    namespace.to_owned(),
                    addr.clone(),
                    authority.to_ascii_lowercase(),
                ))
            }) {
                continue;
            }
            let origins = session
                .evidence
                .performance
                .transports
                .iter()
                .filter(|transport| transport.leg == "upstream")
                .collect::<Vec<_>>();
            for (position, origin) in origins
                .iter()
                .copied()
                .map(Some)
                .chain(origins.is_empty().then_some(None))
                .enumerate()
            {
                let key = origin.map_or_else(
                    || format!("client:{}", client.connection_id),
                    |origin| {
                        if origin.connection_id.is_empty() {
                            format!("unknown:{id}:{position}")
                        } else {
                            format!("upstream:{}", origin.connection_id)
                        }
                    },
                );
                let summary = summaries
                    .entry((namespace.to_owned(), key))
                    .or_insert_with(|| Summary {
                        session: new_summary(session, client, &authority),
                        started: None,
                        observed: None,
                        setup_ended: None,
                    });
                update_summary(summary, session, client, origin);
            }
        }
    }
    summaries.into_values().map(finish_summary).collect()
}

fn destination(client: &TransportObservation, target: &http::Uri) -> Option<String> {
    let authority = client.tunnel_destination.clone().or_else(|| {
        let name = client.tls.as_ref()?.server_name.as_deref()?;
        let host = target.host()?;
        name.eq_ignore_ascii_case(host)
            .then(|| format!("{host}:{}", target.port_u16().unwrap_or(443)))
    })?;
    authority.parse::<http::uri::Authority>().ok()?;
    Some(authority)
}

fn update_summary(
    summary: &mut Summary,
    session: &Session,
    client: &TransportObservation,
    origin: Option<&TransportObservation>,
) {
    summary.session.ordering_unix_nanos = summary
        .session
        .ordering_unix_nanos
        .into_iter()
        .chain(crate::ordering_time(session))
        .min();
    add_transport(&mut summary.session, client);
    if let Some(origin) = origin {
        add_transport(&mut summary.session, origin);
    }
    let started = match origin {
        Some(origin) => origin
            .setup_timings
            .iter()
            .map(|timing| timing.began_offset_micros)
            .min()
            .and_then(|offset| at_offset(session, offset)),
        None => session
            .evidence
            .performance
            .points
            .iter()
            .find(|point| point.milestone == Milestone::ClientConnected)
            .map(|point| i128::from(point.unix_millis) * 1000),
    };
    summary.started = summary.started.into_iter().chain(started).min();
    if let Some(origin) = origin {
        summary.observed = summary
            .observed
            .into_iter()
            .chain(at_offset(session, origin.sampled_offset_micros))
            .max();
        summary.setup_ended = summary
            .setup_ended
            .into_iter()
            .chain(
                origin
                    .setup_timings
                    .iter()
                    .map(|timing| timing.ended_offset_micros)
                    .max()
                    .and_then(|offset| at_offset(session, offset)),
            )
            .max();
    }
}

fn finish_summary(mut summary: Summary) -> Session {
    summary.session.started_unix_nanos = summary
        .started
        .and_then(|micros| micros.checked_mul(1000))
        .and_then(|nanos| u128::try_from(nanos).ok());
    if let Some(unix_millis) = summary
        .started
        .and_then(|micros| u64::try_from(micros / 1000).ok())
    {
        summary
            .session
            .evidence
            .performance
            .points
            .push(transmog_core::performance::TimingPoint {
                milestone: Milestone::RequestHeaders,
                unix_millis,
                offset_micros: 0,
            });
    }
    let status = status(&summary.session);
    let mut text = format!(
        "[Transmog] Recorded connection context: {}. This diagnostic was generated for SAZ export; it is not an HTTP response body from the origin.\r\n\r\n",
        crate::connection_reason(&summary.session)
    );
    for transport in &summary.session.evidence.performance.transports {
        describe(&mut text, transport);
    }
    text.push_str("The socket close time was not recorded. Byte counts are totals at the last recorded observation.\r\n");
    summary.session.response_body = text.into_bytes();
    let mut headers = timing_headers(
        summary.started,
        summary.observed,
        summary.setup_ended,
        status != 200,
    );
    if let Some(origin) = upstream(&summary.session) {
        headers.extend(byte_headers(origin, false));
    }
    let response = summary.session.response.as_mut().unwrap();
    response.status = status;
    response.headers.extend(headers);
    response
        .headers
        .push(crate::length_header(summary.session.response_body.len()));
    summary
        .session
        .request
        .as_mut()
        .unwrap()
        .headers
        .push(crate::length_header(summary.session.request_body.len()));
    summary.session
}

fn at_offset(session: &Session, offset: i64) -> Option<i128> {
    let points = &session.evidence.performance.points;
    let point = points
        .iter()
        .find(|point| point.milestone == Milestone::RequestHeaders)
        .or_else(|| points.first())?;
    let micros =
        i128::from(point.unix_millis) * 1000 + i128::from(offset) - i128::from(point.offset_micros);
    (micros >= 0).then_some(micros)
}

fn upstream(session: &Session) -> Option<&TransportObservation> {
    session
        .evidence
        .performance
        .transports
        .iter()
        .find(|transport| transport.leg == "upstream")
}

fn status(session: &Session) -> u16 {
    match upstream(session)
        .map(|transport| transport.outcome)
        .unwrap_or_default()
    {
        TransportOutcome::Connected => 200,
        TransportOutcome::DnsFailed
        | TransportOutcome::TcpFailed
        | TransportOutcome::TlsFailed
        | TransportOutcome::PolicyRejected => 502,
        TransportOutcome::Unavailable => 599,
    }
}

fn timing_headers(
    started: Option<i128>,
    observed: Option<i128>,
    setup_ended: Option<i128>,
    failed: bool,
) -> Vec<transmog_capture::CapturedHeader> {
    let mut headers = Vec::new();
    for (name, micros) in [
        ("StartTime", started),
        ("Timestamp", failed.then_some(setup_ended).flatten()),
        ("Transmog-Last-Observed", observed),
    ] {
        let Some(time) = micros
            .and_then(|micros| micros.checked_mul(1000))
            .and_then(|nanos| time::OffsetDateTime::from_unix_timestamp_nanos(nanos).ok())
        else {
            continue;
        };
        let value = if name == "Transmog-Last-Observed" {
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                time.year(),
                u8::from(time.month()),
                time.day(),
                time.hour(),
                time.minute(),
                time.second(),
                time.millisecond()
            )
        } else {
            format!(
                "{:02}:{:02}:{:02}.{:03}",
                time.hour(),
                time.minute(),
                time.second(),
                time.millisecond()
            )
        };
        headers.push(header(name, &value));
    }
    if !headers.is_empty() {
        headers.push(header("Transmog-Time-Zone", "UTC"));
    }
    headers
}

fn byte_headers(
    transport: &TransportObservation,
    opaque: bool,
) -> Vec<transmog_capture::CapturedHeader> {
    let mut headers = Vec::new();
    for (name, count) in [
        ("ClientToServerBytes", transport.bytes_written),
        ("ServerToClientBytes", transport.bytes_read),
    ] {
        if let Some(count) = count {
            headers.push(header(name, &count.to_string()));
        }
    }
    if !headers.is_empty() {
        headers.push(header(
            "Transmog-Byte-Counts",
            if opaque {
                "Opaque tunnel bytes at last observation"
            } else {
                "Proxy-to-origin physical bytes at last observation; shared connection totals"
            },
        ));
    }
    headers
}

fn captured_connects(sessions: &BTreeMap<u128, Session>) -> HashSet<(String, String, String)> {
    sessions
        .values()
        .filter_map(|session| {
            let request = session.request.as_ref()?;
            if request.method != "CONNECT" {
                return None;
            }
            let uri = request.target.parse::<http::Uri>().ok()?;
            Some((
                namespace(session).to_owned(),
                session.evidence.client_addr.clone()?,
                format!("{}:{}", uri.host()?, uri.port_u16().unwrap_or(443)).to_ascii_lowercase(),
            ))
        })
        .collect()
}

fn new_summary(session: &Session, client: &TransportObservation, authority: &str) -> Session {
    let mut summary = Session {
        kind: crate::SessionKind::ConnectionContext,
        ordering_unix_nanos: crate::ordering_time(session),
        request: Some(CapturedRequest {
            method: "CONNECT".into(),
            target: format!("https://{authority}/"),
            headers: vec![
                header("Host", authority),
                header(
                    "X-Transmog-Generated",
                    "Connection summary; original CONNECT headers were not captured",
                ),
            ],
        }),
        response: Some(CapturedResponse {
            status: 200,
            headers: vec![header(
                "X-Transmog-Generated",
                "Connection summary; not a captured server response",
            )],
        }),
        terminal: TerminalState::Completed,
        request_body_completion: BodyCompletion::Complete,
        response_body_completion: BodyCompletion::Complete,
        ..Session::default()
    };
    summary
        .evidence
        .client_addr
        .clone_from(&session.evidence.client_addr);
    summary
        .evidence
        .client_identity
        .clone_from(&session.evidence.client_identity);
    if let Some(point) = session
        .evidence
        .performance
        .points
        .iter()
        .find(|point| point.milestone == Milestone::ClientConnected)
    {
        summary.evidence.performance.points.push(point.clone());
    }
    summary.request_body = format!("[Transmog] Connection context generated for SAZ export.\r\nTarget: {authority}\r\nOriginal CONNECT headers and raw ClientHello bytes were not retained.\r\n{}\r\n",
        if client.tunnel_destination.is_some() { "The CONNECT destination was recorded." }
        else { "The destination is inferred from the recorded TLS server name and HTTPS request authority." }).into_bytes();
    summary
}

pub(crate) fn opaque_transport(session: &Session) -> Option<&TransportObservation> {
    if session
        .request
        .as_ref()
        .is_none_or(|request| request.method != "CONNECT")
        || session
            .evidence
            .performance
            .transports
            .iter()
            .any(|transport| {
                transport.leg == "client"
                    && (transport.tls_version.is_some() || transport.tls.is_some())
            })
    {
        return None;
    }
    session
        .evidence
        .performance
        .transports
        .iter()
        .rev()
        .find(|transport| {
            transport.leg == "upstream"
                && transport.tunnel_destination.is_some()
                && transport.tls_version.is_none()
                && transport.tls.is_none()
        })
}

pub(crate) fn opaque_response(
    session: &Session,
    response: &CapturedResponse,
) -> Option<CapturedResponse> {
    if response.status != 200 {
        return None;
    }
    let transport = opaque_transport(session)?;
    let mut headers = vec![header(
        "Transmog-Tunnel",
        "Opaque CONNECT; traffic contents and origin TLS were not inspected",
    )];
    if let Some(peer) = &transport.peer {
        headers.push(header("Transmog-Peer", peer));
    }
    let started = transport
        .setup_timings
        .iter()
        .map(|timing| timing.began_offset_micros)
        .min()
        .and_then(|offset| at_offset(session, offset));
    headers.extend(timing_headers(
        started,
        at_offset(session, transport.sampled_offset_micros),
        None,
        false,
    ));
    headers.extend(byte_headers(transport, true));
    if session.terminal != TerminalState::Open
        && let Some(point) = session
            .evidence
            .performance
            .points
            .iter()
            .find(|point| point.milestone == Milestone::ExchangeDone)
    {
        let mut ended = timing_headers(
            Some(i128::from(point.unix_millis) * 1000),
            None,
            None,
            false,
        );
        for field in &mut ended {
            if field.name == b"StartTime" {
                field.name = b"EndTime".to_vec();
            }
        }
        headers.extend(ended);
        headers.push(header(
            "Transmog-End-Time-Semantics",
            "Local opaque tunnel forwarding ended",
        ));
    }
    let mut names = HashSet::new();
    headers.retain(|field| {
        !response
            .headers
            .iter()
            .any(|original| original.name.eq_ignore_ascii_case(&field.name))
            && names.insert(field.name.to_ascii_lowercase())
    });
    headers.splice(0..0, response.headers.iter().cloned());
    Some(CapturedResponse {
        status: response.status,
        headers,
    })
}

fn namespace(session: &Session) -> &str {
    session
        .evidence
        .provenance
        .as_ref()
        .and_then(|entry| entry["source"]["traceId"].as_str())
        .unwrap_or_default()
}

pub(crate) fn header(name: &str, value: &str) -> transmog_capture::CapturedHeader {
    transmog_capture::CapturedHeader {
        name: name.as_bytes().to_vec(),
        value: Some(value.as_bytes().to_vec()),
        original_value_bytes: None,
    }
}

fn add_transport(summary: &mut Session, transport: &TransportObservation) {
    let transports = &mut summary.evidence.performance.transports;
    if let Some(previous) = transports.iter_mut().find(|previous| {
        previous.leg == transport.leg && previous.connection_id == transport.connection_id
    }) {
        previous.bytes_read = previous
            .bytes_read
            .into_iter()
            .chain(transport.bytes_read)
            .max();
        previous.bytes_written = previous
            .bytes_written
            .into_iter()
            .chain(transport.bytes_written)
            .max();
    } else if transports.len() < 16 {
        transports.push(transport.clone());
    }
}

fn describe(text: &mut String, transport: &TransportObservation) {
    let _ = writeln!(
        text,
        "== {} ==\r",
        if transport.leg == "client" {
            "Client to capturing proxy (interception TLS)"
        } else {
            "Capturing proxy to origin"
        }
    );
    let _ = writeln!(
        text,
        "Connection: {}\r\nRecorded outcome: {:?}\r",
        transport.connection_id, transport.outcome
    );
    for (name, value) in [
        ("Peer", &transport.peer),
        ("Local", &transport.local),
        ("TLS protocol", &transport.tls_version),
        ("Cipher", &transport.cipher),
        ("Negotiated ALPN", &transport.alpn),
    ] {
        if let Some(value) = value {
            let _ = writeln!(text, "{name}: {value}\r");
        }
    }
    for (name, value) in [
        ("Physical bytes read", transport.bytes_read),
        ("Physical bytes written", transport.bytes_written),
    ] {
        if let Some(value) = value {
            let _ = writeln!(text, "{name}: {value} (shared connection total)\r");
        }
    }
    if transport
        .tls
        .as_ref()
        .is_none_or(|tls| !tls.details.contains_key("Session resumed"))
        && let Some(resumed) = transport.tls_resumed
    {
        let _ = writeln!(
            text,
            "Session resumed: {}\r",
            if resumed { "Yes" } else { "No" }
        );
    }
    if let Some(tls) = &transport.tls {
        if let Some(name) = &tls.server_name {
            let _ = writeln!(text, "Server name: {name}\r");
        }
        for (label, _) in crate::tls::NEGOTIATION_FIELDS {
            if let Some(value) = tls.details.get(label) {
                // Imported text cannot forge another diagnostic line or heading.
                let value = crate::xml_text::display_value(value)
                    .replace('\r', "\\r")
                    .replace('\n', "\\n");
                let _ = writeln!(text, "{label}: {value}\r");
            }
        }
        for (name, values) in [
            (
                "Recorded local TLS version configuration",
                &tls.offered_versions,
            ),
            ("Recorded local cipher configuration", &tls.offered_ciphers),
            ("Recorded local ALPN configuration", &tls.offered_alpn),
        ] {
            if !values.is_empty() {
                let _ = writeln!(text, "{name}: {}\r", values.join(", "));
            }
        }
        if let Some(verification) = &tls.verification {
            let _ = writeln!(text, "Recorded verification: {verification:?}\r");
        }
        for error in &tls.errors {
            let _ = writeln!(text, "Recorded TLS error: {error}\r");
        }
        if let Some(certificate) = tls.server_chain.first() {
            certificate_summary(text, certificate);
        }
    }
    text.push_str("\r\n");
}

fn certificate_summary(text: &mut String, certificate: &transmog_core::PublicCertificate) {
    let _ = writeln!(
        text,
        "== Server certificate ==\r\nSHA-256: {}\r",
        certificate.sha256
    );
    let Ok(parsed) = boring::x509::X509::from_der(&certificate.der) else {
        text.push_str("Certificate bytes were not recorded.\r\n");
        return;
    };
    for (label, name) in [
        ("Subject", parsed.subject_name()),
        ("Issuer", parsed.issuer_name()),
    ] {
        let name = name
            .entries()
            .map(|entry| {
                let key = entry.object().nid().short_name().unwrap_or("OID");
                let value = entry
                    .data()
                    .as_utf8()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                format!("{key}={}", value.chars().take(512).collect::<String>())
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(text, "{label}: {name}\r");
    }
    if let Ok(serial) = parsed
        .serial_number()
        .to_bn()
        .and_then(|number| number.to_hex_str())
    {
        let _ = writeln!(text, "Serial: {serial}\r");
    }
    let _ = writeln!(
        text,
        "Not before: {}\r\nNot after: {}\r",
        parsed.not_before(),
        parsed.not_after()
    );
    if let Some(names) = parsed.subject_alt_names() {
        let names = names
            .iter()
            .filter_map(|name| name.dnsname())
            .take(32)
            .collect::<Vec<_>>()
            .join(", ");
        if !names.is_empty() {
            let _ = writeln!(text, "DNS subject alternative names: {names}\r");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use transmog_core::TlsEvidence;

    #[test]
    fn negotiation_values_cannot_inject_summary_lines() {
        let mut sample = evidence();
        sample.transports[0].tls.as_mut().unwrap().details.insert(
            "Negotiated group".into(),
            "X25519\r\nInjected verification".into(),
        );
        let mut text = String::new();
        super::describe(&mut text, &sample.transports[0]);
        assert!(text.contains("Negotiated group: X25519\\r\\nInjected verification"));
        assert!(!text.contains("\r\nInjected verification"));
    }

    #[test]
    fn typed_resumption_is_reported_without_extended_tls_evidence() {
        let mut transport = evidence().transports.remove(0);
        transport.tls = None;
        transport.tls_resumed = Some(true);
        let mut text = String::new();
        super::describe(&mut text, &transport);
        assert!(text.contains("Session resumed: Yes\r\n"));
        assert!(!text.contains("Negotiated group"));
    }

    pub(crate) fn evidence() -> transmog_core::performance::PerformanceEvidence {
        transmog_core::performance::PerformanceEvidence {
            transports: vec![
                TransportObservation {
                    leg: "client".into(),
                    connection_id: "client-fixture".into(),
                    tunnel_destination: Some("example.test:443".into()),
                    tls_version: Some("TLSv1.3".into()),
                    cipher: Some("TLS_AES_128_GCM_SHA256".into()),
                    alpn: Some("h2".into()),
                    bytes_read: Some(200),
                    bytes_written: Some(300),
                    tls: Some(TlsEvidence {
                        server_name: Some("example.test".into()),
                        ..TlsEvidence::default()
                    }),
                    ..TransportObservation::default()
                },
                TransportObservation {
                    leg: "upstream".into(),
                    connection_id: "origin-fixture".into(),
                    outcome: TransportOutcome::Connected,
                    tls_version: Some("TLSv1.3".into()),
                    cipher: Some("TLS_AES_256_GCM_SHA384".into()),
                    peer: Some("192.0.2.1:443".into()),
                    ..TransportObservation::default()
                },
            ],
            ..Default::default()
        }
    }
    #[test]
    fn summaries_deduplicate_connections_and_keep_source_namespaces_separate() {
        let mut sessions = BTreeMap::new();
        for id in 1..=3 {
            let mut session = Session {
                request: Some(CapturedRequest {
                    method: "GET".into(),
                    target: "https://example.test/path".into(),
                    headers: vec![],
                }),
                ..Default::default()
            };
            session.evidence.performance = evidence();
            session.evidence.provenance =
                Some(serde_json::json!({"source":{"traceId":if id==3 {"second"} else {"first"}}}));
            sessions.insert(id, session);
        }
        let summaries = summaries(&sessions);
        assert_eq!(summaries.len(), 2);
        let body = String::from_utf8_lossy(&summaries[0].response_body);
        assert!(body.contains("Client to capturing proxy (interception TLS)"));
        assert!(body.contains("Capturing proxy to origin"));
        assert!(body.contains("TLS_AES_128_GCM_SHA256"));
        assert!(body.contains("TLS_AES_256_GCM_SHA384"));
        assert!(body.contains("Physical bytes read: 200 (shared connection total)"));
        assert!(!body.contains("Fiddler"));
        assert!(summaries[0].started_unix_nanos.is_none());
    }

    #[test]
    fn opaque_tunnel_end_time_requires_a_recorded_terminal_event() {
        use transmog_core::performance::{ConnectionTiming, TimingPoint};
        let mut session = Session {
            request: Some(CapturedRequest {
                method: "CONNECT".into(),
                target: "https://example.test:443/".into(),
                headers: vec![],
            }),
            terminal: TerminalState::Completed,
            ..Default::default()
        };
        session.evidence.performance.points = vec![
            TimingPoint {
                milestone: Milestone::RequestHeaders,
                unix_millis: 1_800_000_000_000,
                offset_micros: 0,
            },
            TimingPoint {
                milestone: Milestone::ExchangeDone,
                unix_millis: 1_800_000_000_010,
                offset_micros: 10000,
            },
        ];
        session
            .evidence
            .performance
            .transports
            .push(TransportObservation {
                leg: "upstream".into(),
                connection_id: "opaque".into(),
                tunnel_destination: Some("example.test:443".into()),
                outcome: TransportOutcome::Connected,
                setup_timings: vec![ConnectionTiming {
                    phase: "tcp".into(),
                    began_offset_micros: 0,
                    ended_offset_micros: 1000,
                    request_wait_micros: 1000,
                }],
                sampled_offset_micros: 10000,
                bytes_read: Some(11),
                bytes_written: Some(22),
                ..Default::default()
            });
        let response = CapturedResponse {
            status: 200,
            headers: vec![],
        };
        let projected = opaque_response(&session, &response).unwrap();
        for (name, value) in [
            ("StartTime", "08:00:00.000"),
            ("EndTime", "08:00:00.010"),
            ("ClientToServerBytes", "22"),
            ("ServerToClientBytes", "11"),
        ] {
            assert!(
                projected
                    .headers
                    .iter()
                    .any(|field| field.name == name.as_bytes()
                        && field.value.as_deref() == Some(value.as_bytes()))
            );
        }
        assert_eq!(
            projected
                .headers
                .iter()
                .filter(|field| field.name == b"Transmog-Time-Zone")
                .count(),
            1
        );
        session.terminal = TerminalState::Open;
        assert!(
            !opaque_response(&session, &response)
                .unwrap()
                .headers
                .iter()
                .any(|field| field.name == b"EndTime")
        );
        session.terminal = TerminalState::Completed;
        session.evidence.performance.points.pop();
        assert!(
            !opaque_response(&session, &response)
                .unwrap()
                .headers
                .iter()
                .any(|field| field.name == b"EndTime")
        );
    }

    fn opaque_fixture() -> Session {
        let mut session = Session {
            request: Some(CapturedRequest {
                method: "CONNECT".into(),
                target: "https://example.test:443/".into(),
                headers: vec![],
            }),
            response: Some(CapturedResponse {
                status: 200,
                headers: vec![],
            }),
            ..Default::default()
        };
        session
            .evidence
            .performance
            .transports
            .push(TransportObservation {
                leg: "upstream".into(),
                connection_id: "opaque-fixture".into(),
                tunnel_destination: Some("example.test:443".into()),
                peer: Some("192.0.2.1:443".into()),
                bytes_read: Some(11),
                bytes_written: Some(22),
                ..Default::default()
            });
        session
    }

    #[test]
    fn tls_evidence_without_a_negotiated_version_prevents_opaque_export() {
        for leg in ["client", "upstream"] {
            let mut session = opaque_fixture();
            let tls = Some(TlsEvidence {
                server_name: Some("example.test".into()),
                ..Default::default()
            });
            if leg == "client" {
                session
                    .evidence
                    .performance
                    .transports
                    .push(TransportObservation {
                        leg: leg.into(),
                        tls,
                        ..Default::default()
                    });
            } else {
                session.evidence.performance.transports[0].tls = tls;
            }
            assert!(opaque_transport(&session).is_none(), "{leg}");
            assert!(
                opaque_response(&session, session.response.as_ref().unwrap()).is_none(),
                "{leg}"
            );
            let metadata = crate::metadata::render(&session, 1, false, false);
            assert!(!metadata.contains("<TunnelInfo"), "{leg}");
            assert!(!metadata.contains("BitFlags=\"1400\""), "{leg}");
        }
    }

    #[test]
    fn opaque_export_preserves_recorded_header_collisions_and_duplicate_fields() {
        let mut session = opaque_fixture();
        let response = session.response.as_mut().unwrap();
        response.headers = vec![
            header("tRaNsMoG-PeEr", "first recorded peer"),
            header("Transmog-Peer", "second recorded peer"),
            header("CLIENTTOSERVERBYTES", "recorded sent count"),
            header("Transmog-Tunnel", "recorded tunnel context"),
        ];
        let response = session.response.as_ref().unwrap();
        let exported = opaque_response(&session, response).unwrap();
        for (original, retained) in response.headers.iter().zip(&exported.headers) {
            assert_eq!(original.name, retained.name);
            assert_eq!(original.value, retained.value);
        }
        assert_eq!(exported.headers.len(), response.headers.len() + 2);
        assert!(
            exported
                .headers
                .iter()
                .skip(response.headers.len())
                .all(|field| field.name == b"ServerToClientBytes"
                    || field.name == b"Transmog-Byte-Counts")
        );
        let metadata = crate::metadata::render(&session, 1, false, false);
        assert!(metadata.contains(
            "N=\"x-transmog-connect-export-headers\" V=\"ServerToClientBytes,Transmog-Byte-Counts\""
        ));
    }

    #[test]
    fn public_certificate_summary_uses_original_der_without_current_trust_evaluation() {
        let parsed = boring::x509::X509::from_pem(include_bytes!(
            "../../app/tests/fixtures/certificates/leaf.pem"
        ))
        .unwrap();
        let digest = parsed
            .digest(boring::hash::MessageDigest::sha256())
            .unwrap();
        let certificate = transmog_core::PublicCertificate {
            sha256: digest.iter().fold(String::new(), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            }),
            der: parsed.to_der().unwrap().into(),
        };
        let mut text = String::new();
        certificate_summary(&mut text, &certificate);
        assert!(text.contains("Subject: CN=localhost"));
        assert!(text.contains("Issuer:"));
        assert!(text.contains("Serial:"));
        assert!(text.contains("Not before:"));
        assert!(text.contains("Not after:"));
        assert!(text.contains(&certificate.sha256));
        assert!(!text.contains("PRIVATE KEY"));
    }
}
