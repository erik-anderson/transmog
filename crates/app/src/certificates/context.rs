//! Presentation of recorded TLS facts; capture evidence is never rewritten.
use super::{CertificateConnection, field};
use std::{collections::BTreeMap, fmt::Write as _};
use transmog_core::performance::{TransportObservation, TransportOutcome};

fn invisible(character: char) -> bool {
    (character.is_control() && !matches!(character, '\n' | '\t'))
        || matches!(character, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}' | '\u{fffe}' | '\u{ffff}')
}

pub(super) fn display_text(value: &str) -> String {
    if !value.chars().any(invisible) {
        return value.to_owned();
    }
    let mut display = String::new();
    for character in value.chars() {
        if invisible(character) {
            let _ = write!(display, "\\u{{{:04x}}}", u32::from(character));
        } else if character == '\\' {
            display.push_str("\\\\");
        } else {
            display.push(character);
        }
    }
    display
}

pub(super) fn display_note(value: &str) -> String {
    if value.chars().any(invisible) {
        "Invisible characters shown as Unicode escapes.".into()
    } else {
        String::new()
    }
}

pub(super) const fn outcome_label(outcome: TransportOutcome) -> &'static str {
    match outcome {
        TransportOutcome::Connected => "Connected",
        TransportOutcome::TlsFailed => "TLS failed",
        TransportOutcome::TcpFailed => "TCP failed",
        TransportOutcome::DnsFailed => "DNS failed",
        TransportOutcome::PolicyRejected => "Rejected by policy",
        TransportOutcome::Unavailable => "Outcome unavailable",
    }
}

const ORDERED_DETAILS: [&str; 10] = [
    "Session resumed",
    "Key exchange",
    "Negotiated group",
    "Negotiated group ID",
    "Original session group",
    "Original session group ID",
    "Local handshake signature",
    "Peer handshake signature",
    "Original peer handshake signature",
    "HelloRetryRequest",
];

fn negotiated<'a>(
    connection: &mut CertificateConnection,
    details: &'a BTreeMap<String, String>,
    label: &str,
    fallback_label: &str,
    observed: Option<&'a str>,
) -> Option<&'a str> {
    let diagnostic = details.get(label).map(String::as_str);
    let value = observed.or(diagnostic)?;
    connection.fields.push(field(
        if diagnostic.is_some() {
            label
        } else {
            fallback_label
        },
        value,
    ));
    if diagnostic.is_some_and(|diagnostic| diagnostic != value) {
        connection
            .diagnostic_fields
            .push(field(label, diagnostic.unwrap()));
        connection.errors.push(format!(
            "Recorded {fallback_label} values disagree; additional metadata retains the other value."
        ));
    }
    Some(value)
}

pub(super) fn organize(connection: &mut CertificateConnection, transport: &TransportObservation) {
    let original = std::mem::take(&mut connection.fields);
    connection.policy_fields.splice(
        0..0,
        original
            .iter()
            .filter(|field| matches!(field.label.as_str(), "Connection" | "Peer"))
            .cloned(),
    );
    connection.fields.extend(
        original
            .iter()
            .filter(|field| matches!(field.label.as_str(), "Outcome" | "Request result"))
            .cloned(),
    );
    let empty = BTreeMap::new();
    let details = transport.tls.as_ref().map_or(&empty, |tls| &tls.details);
    let version = negotiated(
        connection,
        details,
        "Negotiated TLS version",
        "TLS version",
        transport.tls_version.as_deref(),
    );
    append_exchange(connection, transport, details, version.is_some());
    let cipher = negotiated(
        connection,
        details,
        "Negotiated cipher",
        "Cipher",
        transport.cipher.as_deref(),
    );
    if let Some(alpn) = &transport.alpn {
        connection.fields.push(field("ALPN", alpn));
    }
    super::alps::organize(connection, transport);
    if let Some(value) = details.get("HelloRetryRequest") {
        connection.fields.push(field("HelloRetryRequest", value));
    }
    connection.fields.extend(
        original
            .into_iter()
            .filter(|field| field.label == "Recorded verification"),
    );
    connection.diagnostic_fields.extend(
        details
            .iter()
            .filter(|(label, _)| {
                !ORDERED_DETAILS.contains(&label.as_str())
                    && !matches!(
                        label.as_str(),
                        "Negotiated TLS version" | "Negotiated cipher" | "HTTP/2 ALPS"
                    )
            })
            .map(|(label, value)| field(label, value)),
    );
    connection.summary = summary(transport, details, version, cipher);
    warn_imported_conflicts(connection, transport, details, version, cipher);
}

fn append_exchange(
    connection: &mut CertificateConnection,
    transport: &TransportObservation,
    details: &BTreeMap<String, String>,
    has_version: bool,
) {
    negotiated(
        connection,
        details,
        "Session resumed",
        "Session resumed",
        transport
            .tls_resumed
            .map(|resumed| if resumed { "Yes" } else { "No" }),
    );
    for label in ORDERED_DETAILS {
        if matches!(label, "Session resumed" | "HelloRetryRequest") {
            continue;
        }
        if let Some(value) = details.get(label) {
            connection.fields.push(field(label, value));
        } else if label == "Key exchange" && (transport.tls.is_some() || has_version) {
            connection.fields.push(field(label, "Unavailable"));
        }
    }
}

fn summary(
    transport: &TransportObservation,
    details: &BTreeMap<String, String>,
    version: Option<&str>,
    cipher: Option<&str>,
) -> String {
    let mut summary = [
        version,
        details.get("Negotiated group").map(String::as_str),
        cipher,
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    if transport.tls_resumed.unwrap_or_else(|| {
        details
            .get("Session resumed")
            .is_some_and(|value| value == "Yes")
    }) {
        summary.push("Resumed");
    }
    if summary.is_empty() {
        "TLS negotiation unavailable".into()
    } else {
        display_text(&summary.join(" · "))
    }
}

fn warn_imported_conflicts(
    connection: &mut CertificateConnection,
    transport: &TransportObservation,
    details: &BTreeMap<String, String>,
    version: Option<&str>,
    cipher: Option<&str>,
) {
    let prefix = if transport.leg == "client" {
        "https-client-"
    } else {
        "https-server-"
    };
    for (suffix, observed) in [
        ("protocol", version),
        ("cipher", cipher),
        (
            "sni",
            transport
                .tls
                .as_ref()
                .and_then(|tls| tls.server_name.as_deref()),
        ),
    ] {
        let Some(observed) = observed else {
            continue;
        };
        let name = format!("{prefix}{suffix}");
        if details
            .iter()
            .any(|(label, value)| label.eq_ignore_ascii_case(&name) && value != observed)
        {
            connection.errors.push(format!("Imported {name} differs from the connection's recorded value; additional metadata preserves the original."));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_core::TlsEvidence;

    fn connection() -> CertificateConnection {
        CertificateConnection {
            id: "upstream:origin".into(),
            title: "Transmog → server".into(),
            summary: String::new(),
            fields: vec![
                field("Connection", "origin"),
                field("Outcome", "Connected"),
                field(
                    "Recorded verification",
                    "Verified by the original connection",
                ),
            ],
            policy_fields: vec![field("Available cipher suites", "configured only")],
            diagnostic_fields: vec![],
            alps: None,
            server_chain: vec![],
            client_chain: vec![],
            errors: vec![],
        }
    }

    #[test]
    fn negotiation_is_ordered_deduplicated_and_separate_from_policy() {
        let mut view = connection();
        let transport = TransportObservation {
            tls_version: Some("TLSv1.3".into()),
            cipher: Some("TLS_AES_128_GCM_SHA256".into()),
            tls_resumed: Some(false),
            tls: Some(TlsEvidence {
                details: BTreeMap::from([
                    ("Negotiated TLS version".into(), "TLSv1.3".into()),
                    ("Negotiated cipher".into(), "TLS_AES_128_GCM_SHA256".into()),
                    (
                        "Key exchange".into(),
                        "Hybrid post-quantum key exchange".into(),
                    ),
                    ("Negotiated group".into(), "X25519MLKEM768".into()),
                    ("Peer handshake signature".into(), "mldsa44".into()),
                    ("Session resumed".into(), "No".into()),
                    (
                        "https-server-cipher".into(),
                        "TLS_AES_128_GCM_SHA256".into(),
                    ),
                    (
                        "Recorded verification".into(),
                        "untrusted metadata label".into(),
                    ),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        organize(&mut view, &transport);
        assert_eq!(
            view.summary,
            "TLSv1.3 · X25519MLKEM768 · TLS_AES_128_GCM_SHA256"
        );
        let labels = view
            .fields
            .iter()
            .map(|field| field.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "Outcome",
                "Negotiated TLS version",
                "Session resumed",
                "Key exchange",
                "Negotiated group",
                "Peer handshake signature",
                "Negotiated cipher",
                "HTTP/2 ALPS",
                "Recorded verification"
            ]
        );
        assert_eq!(view.policy_fields[0].label, "Connection");
        assert!(
            view.diagnostic_fields
                .iter()
                .any(|field| field.label == "https-server-cipher")
        );
        assert_eq!(
            view.fields.last().unwrap().value,
            "Verified by the original connection"
        );
        assert!(
            view.diagnostic_fields
                .iter()
                .any(|field| field.value == "untrusted metadata label")
        );
    }

    #[test]
    fn conflicting_protocol_and_resumption_remain_visible_without_overriding_observations() {
        let mut view = connection();
        let transport = TransportObservation {
            leg: "upstream".into(),
            tls_version: Some("TLSv1.3".into()),
            tls_resumed: Some(false),
            tls: Some(TlsEvidence {
                details: BTreeMap::from([
                    ("Negotiated TLS version".into(), "TLSv1.2".into()),
                    ("Session resumed".into(), "Yes".into()),
                    ("HTTPS-SERVER-PROTOCOL".into(), "TLSv1.2".into()),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        organize(&mut view, &transport);
        assert_eq!(view.summary, "TLSv1.3");
        assert_eq!(view.errors.len(), 3);
        assert!(
            view.diagnostic_fields
                .iter()
                .any(|field| field.value == "TLSv1.2")
        );
        assert!(
            view.diagnostic_fields
                .iter()
                .any(|field| field.value == "Yes")
        );
        assert!(
            view.fields
                .iter()
                .any(|field| field.label == "Session resumed" && field.value == "No")
        );
    }

    #[test]
    fn original_session_group_is_never_a_current_summary_group() {
        let mut view = connection();
        let transport = TransportObservation {
            tls_version: Some("TLSv1.2".into()),
            tls_resumed: Some(true),
            tls: Some(TlsEvidence {
                details: BTreeMap::from([
                    ("Original session group".into(), "P-256".into()),
                    (
                        "Key exchange".into(),
                        "Not performed (TLS 1.2 session resumed)".into(),
                    ),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        organize(&mut view, &transport);
        assert_eq!(view.summary, "TLSv1.2 · Resumed");
        assert!(
            !view
                .fields
                .iter()
                .any(|field| field.label == "Negotiated group")
        );
        let mut older = connection();
        organize(
            &mut older,
            &TransportObservation {
                tls: Some(TlsEvidence::default()),
                ..Default::default()
            },
        );
        assert_eq!(older.summary, "TLS negotiation unavailable");
        assert!(
            older
                .fields
                .iter()
                .any(|field| field.label == "Key exchange" && field.value == "Unavailable")
        );
    }

    #[test]
    fn invisible_characters_are_projected_without_changing_original_values() {
        let original = "literal\\u{0000}: X25519\0MLKEM768\u{202e}\u{ffff}";
        let value = field("Negotiated group", original);
        assert_eq!(value.value, original);
        assert_eq!(
            value.display_value,
            "literal\\\\u{0000}: X25519\\u{0000}MLKEM768\\u{202e}\\u{ffff}"
        );
        assert_eq!(
            value.display_note,
            "Invisible characters shown as Unicode escapes."
        );
        let nul = field("Negotiated group", "X25519\0MLKEM768");
        assert_eq!(nul.display_value, "X25519\\u{0000}MLKEM768");
        assert_eq!(nul.value, "X25519\0MLKEM768");
        assert_eq!(
            nul.display_note,
            "Invisible characters shown as Unicode escapes."
        );
        assert_eq!(display_text("literal\\u{0000}"), "literal\\u{0000}");
        assert_eq!(display_text("multiline\nvalue\t🦀"), "multiline\nvalue\t🦀");
        assert_eq!(display_text("malicious\0label"), "malicious\\u{0000}label");
        let label = field("malicious\0label", "unchanged");
        assert_eq!(label.label, "malicious\\u{0000}label");
        assert!(
            label
                .display_note
                .starts_with("Label: Invisible characters")
        );
    }
}
