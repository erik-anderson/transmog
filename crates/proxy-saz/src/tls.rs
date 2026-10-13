//! Negotiated facts carried in conventional `SessionFlag` members.
use std::collections::BTreeMap;

pub(crate) const NEGOTIATION_FIELDS: [(&str, &str); 12] = [
    ("Key exchange", "key-exchange"),
    ("Negotiated group", "group"),
    ("Negotiated group ID", "group-id"),
    ("Local handshake signature", "local-signature"),
    ("Peer handshake signature", "peer-signature"),
    (
        "Original peer handshake signature",
        "original-peer-signature",
    ),
    ("Original session group", "original-session-group"),
    ("Original session group ID", "original-session-group-id"),
    ("Session resumed", "session-resumed"),
    ("HelloRetryRequest", "hello-retry-request"),
    ("Negotiated TLS version", "version"),
    ("Negotiated cipher", "cipher"),
];

/// Restores recorded negotiation facts from conventional SAZ metadata.
/// Values remain original evidence, without inferring algorithms or verification.
/// Only the `client` and `upstream` legs are supported. Unknown flags are ignored.
pub fn tls_negotiation_details(
    flags: &BTreeMap<String, String>,
    leg: &str,
) -> BTreeMap<String, String> {
    let Some(prefix) = flag_prefix(leg) else {
        return BTreeMap::new();
    };
    let mut details = NEGOTIATION_FIELDS
        .iter()
        .filter_map(|(label, suffix)| {
            let name = format!("{prefix}{suffix}");
            let mut values = flags
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(&name))
                .map(|(_, value)| value);
            let value = values.next()?;
            // Ambiguous casing aliases cannot establish a negotiated fact.
            (value.len() <= 4096 && values.all(|other| other == value))
                .then(|| ((*label).to_owned(), value.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    if !details.is_empty() && crate::xml_text::marked(flags) {
        details.insert(
            "SAZ metadata text".into(),
            "Original text restored from visible Unicode escapes in XML metadata.".into(),
        );
    }
    details
}

pub(crate) fn flag_prefix(leg: &str) -> Option<&'static str> {
    match leg {
        "client" => Some("x-transmog-client-tls-"),
        "upstream" => Some("x-transmog-server-tls-"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_facts_are_bounded_unambiguous_and_leg_specific() {
        let flags = BTreeMap::from([
            (
                "X-Transmog-Client-TLS-Group".into(),
                "X25519MLKEM768".into(),
            ),
            ("x-transmog-server-tls-group".into(), "X25519".into()),
            (
                "x-transmog-server-tls-peer-signature".into(),
                "a".repeat(4097),
            ),
            ("x-transmog-client-tls-unknown".into(), "ignored".into()),
        ]);
        assert_eq!(
            tls_negotiation_details(&flags, "client")["Negotiated group"],
            "X25519MLKEM768"
        );
        assert_eq!(
            tls_negotiation_details(&flags, "upstream"),
            BTreeMap::from([("Negotiated group".into(), "X25519".into())])
        );
        assert!(tls_negotiation_details(&flags, "other").is_empty());
        let mut conflicting = flags;
        conflicting.insert("x-transmog-client-tls-group".into(), "X25519".into());
        assert!(tls_negotiation_details(&conflicting, "client").is_empty());
        assert!(
            tls_negotiation_details(
                &BTreeMap::from([(crate::xml_text::FLAG.into(), crate::xml_text::MODE.into())]),
                "client"
            )
            .is_empty()
        );
    }
}
