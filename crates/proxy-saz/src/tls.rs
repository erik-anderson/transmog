//! Negotiated facts carried in conventional `SessionFlag` members.
use std::collections::BTreeMap;

pub(crate) const NEGOTIATION_FIELDS: [(&str, &str); 20] = [
    ("Trust Anchor IDs", "trust-anchor-ids"),
    ("Requested Trust Anchor IDs", "requested-trust-anchor-ids"),
    (
        "Server available Trust Anchor IDs",
        "available-trust-anchor-ids",
    ),
    ("PKI metadata source", "pki-metadata-source"),
    ("PKI metadata version", "pki-metadata-version"),
    ("PKI metadata SHA-256", "pki-metadata-sha256"),
    ("Trust Anchor ID recovery", "trust-anchor-recovery"),
    ("HTTP/2 ALPS", "alps"),
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

const ALPS_CHUNK_BYTES: usize = 4096;

/// Preserves the exact public ALPS value in bounded conventional SAZ flags.
/// Hex chunks fit the existing XML attribute limits, including the maximum TLS value.
pub(crate) fn alps_flags(
    evidence: &transmog_core::Http2AlpsEvidence,
    prefix: &str,
) -> BTreeMap<String, String> {
    use std::fmt::Write as _;
    let mut flags = BTreeMap::from([
        (
            format!("{prefix}alps-negotiated"),
            evidence.negotiated.to_string(),
        ),
        (
            format!("{prefix}alps-peer-bytes"),
            evidence.peer_settings.len().to_string(),
        ),
    ]);
    for (index, chunk) in evidence.peer_settings.chunks(ALPS_CHUNK_BYTES).enumerate() {
        let mut encoded = String::with_capacity(chunk.len() * 2);
        for byte in chunk {
            let _ = write!(encoded, "{byte:02x}");
        }
        flags.insert(format!("{prefix}alps-payload-{index:04}"), encoded);
    }
    flags
}

fn unique_flag<'a>(
    flags: &'a BTreeMap<String, String>,
    name: &str,
    maximum: usize,
) -> Option<&'a str> {
    let mut values = flags
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str());
    let value = values.next()?;
    (value.len() <= maximum && values.all(|other| other == value)).then_some(value)
}

/// Restores bounded raw ALPS evidence. Missing chunks, casing conflicts and
/// inconsistent byte counts cannot establish payload contents.
pub fn tls_alps(
    flags: &BTreeMap<String, String>,
    leg: &str,
) -> Option<transmog_core::Http2AlpsEvidence> {
    let prefix = flag_prefix(leg)?;
    let negotiated = match unique_flag(flags, &format!("{prefix}alps-negotiated"), 5)? {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    let bytes = unique_flag(flags, &format!("{prefix}alps-peer-bytes"), 5)?
        .parse::<usize>()
        .ok()?;
    if bytes > 65_535 || !negotiated && bytes != 0 {
        return None;
    }
    let chunks = bytes.div_ceil(ALPS_CHUNK_BYTES);
    let chunk_prefix = format!("{prefix}alps-payload-");
    for key in flags.keys() {
        let key = key.to_ascii_lowercase();
        if let Some(suffix) = key.strip_prefix(&chunk_prefix) {
            let index = suffix.parse::<usize>().ok()?;
            if suffix != format!("{index:04}") || index >= chunks {
                return None;
            }
        }
    }
    let mut raw = Vec::with_capacity(bytes);
    for index in 0..chunks {
        let encoded = unique_flag(
            flags,
            &format!("{chunk_prefix}{index:04}"),
            ALPS_CHUNK_BYTES * 2,
        )?;
        let expected = (bytes - raw.len()).min(ALPS_CHUNK_BYTES);
        if encoded.len() != expected * 2 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        for pair in encoded.as_bytes().chunks_exact(2) {
            let text = std::str::from_utf8(pair).ok()?;
            raw.push(u8::from_str_radix(text, 16).ok()?);
        }
    }
    Some(transmog_core::Http2AlpsEvidence {
        negotiated,
        peer_settings: raw.into(),
    })
}

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

/// Restores the effective recipe recorded on the original physical connection.
/// Invalid or conflicting aliases remain unavailable rather than inventing provenance.
#[must_use]
pub fn tls_network_profile(
    flags: &BTreeMap<String, String>,
    leg: &str,
) -> Option<transmog_core::network_profile::NetworkProfileSnapshot> {
    let name = format!("{}network-profile", flag_prefix(leg)?);
    let mut values = flags
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(&name))
        .map(|(_, value)| value);
    let value = values.next()?;
    if value.len() > 4096 || values.any(|other| other != value) {
        return None;
    }
    let profile: transmog_core::network_profile::NetworkProfileSnapshot =
        serde_json::from_str(value).ok()?;
    profile.valid().then_some(profile)
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
    fn trust_anchor_metadata_facts_survive_conventional_flags() {
        let flags = BTreeMap::from([
            (
                "x-transmog-server-tls-trust-anchor-ids".into(),
                "Server reported a matching certificate chain".into(),
            ),
            (
                "x-transmog-server-tls-requested-trust-anchor-ids".into(),
                "04d6790901".into(),
            ),
            (
                "x-transmog-server-tls-available-trust-anchor-ids".into(),
                "04d6790901".into(),
            ),
            (
                "x-transmog-server-tls-pki-metadata-source".into(),
                "C:\\Chrome data\\1804\\crs.pb".into(),
            ),
            (
                "x-transmog-server-tls-pki-metadata-version".into(),
                "42".into(),
            ),
            (
                "x-transmog-server-tls-pki-metadata-sha256".into(),
                "a".repeat(64),
            ),
            (
                "x-transmog-server-tls-trust-anchor-recovery".into(),
                "One retry".into(),
            ),
        ]);
        let parsed = tls_negotiation_details(&flags, "upstream");
        assert_eq!(parsed.len(), 7);
        assert_eq!(parsed["Requested Trust Anchor IDs"], "04d6790901");
        assert_eq!(parsed["PKI metadata version"], "42");
        assert_eq!(parsed["PKI metadata SHA-256"], "a".repeat(64));
    }

    #[test]
    fn alps_flags_preserve_maximum_bytes_and_require_complete_unambiguous_hex() {
        for size in [0, 1, 4096, 4097, 65_535] {
            let evidence = transmog_core::Http2AlpsEvidence {
                negotiated: true,
                peer_settings: (0..size)
                    .map(|index| u8::try_from(index % 256).unwrap())
                    .collect::<Vec<_>>()
                    .into(),
            };
            let flags = alps_flags(&evidence, flag_prefix("upstream").unwrap());
            assert!(flags.values().all(|value| value.len() <= 8192));
            assert_eq!(tls_alps(&flags, "upstream"), Some(evidence));
            assert!(tls_alps(&flags, "client").is_none());
            let mut broken = flags.clone();
            broken.insert("X-TRANSMOG-SERVER-TLS-ALPS-PEER-BYTES".into(), "9".into());
            assert!(tls_alps(&broken, "upstream").is_none());
            if size != 0 {
                let mut missing = flags.clone();
                missing.remove("x-transmog-server-tls-alps-payload-0000");
                assert!(tls_alps(&missing, "upstream").is_none());
            }
        }
        let evidence = transmog_core::Http2AlpsEvidence {
            negotiated: false,
            peer_settings: vec![].into(),
        };
        let mut flags = alps_flags(&evidence, flag_prefix("upstream").unwrap());
        assert_eq!(tls_alps(&flags, "upstream"), Some(evidence));
        flags.insert(
            "x-transmog-server-tls-alps-payload-0000".into(),
            "00".into(),
        );
        assert!(tls_alps(&flags, "upstream").is_none());
        flags.insert(
            "x-transmog-server-tls-alps-negotiated".into(),
            "true".into(),
        );
        flags.insert("x-transmog-server-tls-alps-peer-bytes".into(), "1".into());
        flags.insert(
            "x-transmog-server-tls-alps-payload-0000".into(),
            "+f".into(),
        );
        assert!(tls_alps(&flags, "upstream").is_none());
    }

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
