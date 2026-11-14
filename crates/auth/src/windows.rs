//! Dispatch Windows authentication tokens and retain their original representations.
mod asn1;
mod kerberos;
mod ntlm;
mod spnego;

use crate::limits::MAX_HEX_PREVIEW_BYTES;
use crate::{Context, Finding, Source};
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// Recognize a possible raw mechanism envelope without claiming a valid token.
pub(crate) fn is_binary_candidate(bytes: &[u8]) -> bool {
    bytes.starts_with(ntlm::SIGNATURE)
        || bytes.first().is_some_and(|first| {
            [
                asn1::tag::GSS_INITIAL_CONTEXT,
                spnego::NEGOTIATION_INIT,
                spnego::NEGOTIATION_RESPONSE,
                asn1::tag::KERBEROS_AP_REQUEST,
                asn1::tag::KERBEROS_AP_REPLY,
                asn1::tag::KERBEROS_ERROR,
            ]
            .contains(first)
        })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}
pub(crate) fn decode(
    context: &mut Context,
    bytes: &[u8],
    source: Source,
    explicit_ntlm: bool,
) -> Finding {
    let mut finding = if explicit_ntlm {
        ntlm::decode(context, bytes, source)
    } else {
        spnego::decode(context, bytes, source, 0)
    };
    if context.reserve_bytes(bytes.len()) {
        finding.content.push((
            "mechanismBytes".into(),
            crate::json::Node::Bytes(bytes.to_vec()),
        ));
    } else {
        finding.issue(
            "limited",
            "Mechanism bytes exceeded the retained document budget.",
        );
    }
    context.technical_field(&mut finding, "Decoded mechanism bytes (hex prefix)", hex(&bytes[..bytes.len().min(MAX_HEX_PREVIEW_BYTES)]), true, "At most the first 4 KiB of decoded mechanism bytes; full encoded material remains in the enclosing credential/source. Includes sensitive cryptographic data.");
    context.token(
        &mut finding,
        &STANDARD.encode(bytes),
        if explicit_ntlm {
            "ntlm-decode"
        } else {
            "negotiate-decode"
        },
    );
    finding
}
