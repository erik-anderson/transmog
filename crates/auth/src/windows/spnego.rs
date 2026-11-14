//! GSS-API and SPNEGO wrappers preserve offered and selected mechanism semantics.
use super::asn1::tag;
use super::hex;
use super::{
    asn1::{ValidationError, children, context_field, int, oid, tlv, validate},
    kerberos, ntlm,
};
use crate::{Context, Finding, MAX_DEPTH, Source};
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub(super) const NEGOTIATION_INIT: u8 = 0xa0;
pub(super) const NEGOTIATION_RESPONSE: u8 = 0xa1;
const MECHANISM_TOKEN_FIELD: u8 = 0xa2;
const MECHANISM_LIST_MIC_FIELD: u8 = 0xa3;
const KERBEROS_TOKEN_IDS: [[u8; 2]; 3] = [[1, 0], [2, 0], [3, 0]];

mod init_field {
    pub const MECHANISM_TYPES: u8 = 0xa0;
    pub const REQUEST_FLAGS: u8 = 0xa1;
}
mod response_field {
    pub const NEGOTIATION_STATE: u8 = 0xa0;
    pub const SELECTED_MECHANISM: u8 = 0xa1;
}

fn mechanism(value: &str) -> &'static str {
    match value {
        "1.3.6.1.5.5.2" => "SPNEGO",
        "1.2.840.113554.1.2.2" | "1.2.840.48018.1.2.2" => "Kerberos V5",
        "1.3.6.1.4.1.311.2.2.10" => "NTLM",
        _ => "Unknown mechanism",
    }
}

fn validate_envelope(context: &Context, bytes: &[u8], finding: &mut Finding) -> bool {
    let Err(error) = validate(bytes, context) else {
        return true;
    };
    match error {
        ValidationError::Malformed => finding.issue("malformed", "Invalid ASN.1 lengths or tags."),
        ValidationError::Limited => finding.issue(
            "limited",
            "ASN.1 child, node or nesting limit reached; coverage is partial.",
        ),
        ValidationError::Canceled => finding.issue("incomplete", "ASN.1 inspection was canceled."),
    }
    false
}

pub(super) fn decode(
    context: &mut Context,
    bytes: &[u8],
    mut source: Source,
    depth: usize,
) -> Finding {
    if bytes.starts_with(ntlm::SIGNATURE) {
        return ntlm::decode(context, bytes, source);
    }
    let mut finding = Finding::new("GSS-API / SPNEGO", "negotiation token", source.clone());
    finding.verification = "Not checked".into();
    if depth > MAX_DEPTH || context.is_canceled() {
        finding.issue("limited", "Nested mechanism limit exceeded.");
        return finding;
    }
    let Some(outer) = tlv(bytes).filter(|v| v.rest.is_empty()) else {
        finding.issue(
            "malformed",
            "Invalid ASN.1 token envelope, indefinite length or trailing bytes.",
        );
        return finding;
    };
    if outer.tag == tag::GSS_INITIAL_CONTEXT {
        let Some(mech) = tlv(outer.bytes).filter(|v| v.tag == tag::OBJECT_IDENTIFIER) else {
            finding.issue("malformed", "GSS token has no mechanism OID.");
            return finding;
        };
        let Some(value) = oid(mech.bytes) else {
            finding.issue("malformed", "Invalid mechanism OID.");
            return finding;
        };
        context.field(
            &mut finding,
            "GSS mechanism",
            format!("{value} · {}", mechanism(&value)),
            false,
            "Observed wrapper OID; offered mechanisms are not proof of selection.",
        );
        source
            .transforms
            .push(format!("GSS-API {}", mechanism(&value)));
        if mechanism(&value) == "Kerberos V5" {
            let payload = if KERBEROS_TOKEN_IDS
                .iter()
                .any(|id| mech.rest.starts_with(id))
            {
                &mech.rest[size_of::<u16>()..]
            } else {
                mech.rest
            };
            if validate_envelope(context, payload, &mut finding) {
                finding
                    .children
                    .push(kerberos::decode(context, payload, source));
            }
        } else if mechanism(&value) == "SPNEGO" {
            finding
                .children
                .push(decode(context, mech.rest, source, depth + 1));
        } else if mechanism(&value) == "NTLM" {
            finding
                .children
                .push(ntlm::decode(context, mech.rest, source));
        } else {
            finding.issue("unsupported", "Unknown GSS mechanism remains opaque.");
        }
        return finding;
    }
    if !validate_envelope(context, bytes, &mut finding) {
        return finding;
    }
    if [
        tag::KERBEROS_AP_REQUEST,
        tag::KERBEROS_AP_REPLY,
        tag::KERBEROS_ERROR,
    ]
    .contains(&outer.tag)
    {
        return kerberos::decode(context, bytes, source);
    }
    if outer.tag != NEGOTIATION_INIT && outer.tag != NEGOTIATION_RESPONSE {
        finding.issue("unsupported", "Unknown Negotiate token variant.");
        return finding;
    }
    finding.format = "SPNEGO".into();
    finding.role = if outer.tag == NEGOTIATION_INIT {
        "NegTokenInit"
    } else {
        "NegTokenResp"
    }
    .into();
    let Some(sequence) = tlv(outer.bytes).filter(|v| v.tag == tag::SEQUENCE) else {
        finding.issue("malformed", "SPNEGO choice does not contain a sequence.");
        return finding;
    };
    let Some(items) = children(sequence.bytes) else {
        return finding;
    };
    if outer.tag == NEGOTIATION_INIT {
        if let Some(sequence) = context_field(&items, init_field::MECHANISM_TYPES)
            .and_then(tlv)
            .filter(|v| v.tag == tag::SEQUENCE)
            && let Some(mechs) = children(sequence.bytes)
        {
            for mech in mechs {
                if mech.tag == tag::OBJECT_IDENTIFIER
                    && let Some(value) = oid(mech.bytes)
                {
                    context.field(
                        &mut finding,
                        "Offered mechanism",
                        format!("{value} · {}", mechanism(&value)),
                        false,
                        "Offered does not mean selected or observed in the child token.",
                    );
                }
            }
        }
        if let Some(flags) = context_field(&items, init_field::REQUEST_FLAGS)
            .and_then(tlv)
            .filter(|v| v.tag == tag::BIT_STRING)
        {
            context.field(
                &mut finding,
                "Requested context flags",
                hex(flags.bytes),
                false,
                "Observed SPNEGO BIT STRING.",
            );
        }
    } else {
        if let Some(value) = context_field(&items, response_field::NEGOTIATION_STATE).and_then(int)
        {
            context.field(
                &mut finding,
                "Negotiation state",
                format!(
                    "{value} · {}",
                    match value {
                        0 => "accept-completed",
                        1 => "accept-incomplete",
                        2 => "reject",
                        3 => "request-mic",
                        _ => "unknown",
                    }
                ),
                false,
                "Observed SPNEGO state; not an independent verification of authentication.",
            );
        }
        if let Some(value) = context_field(&items, response_field::SELECTED_MECHANISM)
            .and_then(tlv)
            .filter(|v| v.tag == tag::OBJECT_IDENTIFIER)
            .and_then(|v| oid(v.bytes))
        {
            context.field(
                &mut finding,
                "Selected mechanism",
                format!("{value} · {}", mechanism(&value)),
                false,
                "Explicit supportedMech observation, independent of offered mechanisms.",
            );
        }
    }
    if let Some(value) = context_field(&items, MECHANISM_TOKEN_FIELD)
        .and_then(tlv)
        .filter(|v| v.tag == tag::OCTET_STRING)
    {
        source
            .transforms
            .push("SPNEGO mechanism OCTET STRING".into());
        finding
            .children
            .push(decode(context, value.bytes, source, depth + 1));
    }
    if let Some(value) = context_field(&items, MECHANISM_LIST_MIC_FIELD)
        .and_then(tlv)
        .filter(|v| v.tag == tag::OCTET_STRING)
    {
        context.technical_field(
            &mut finding,
            "Mechanism-list MIC",
            STANDARD.encode(value.bytes),
            true,
            "MIC is present; not checked.",
        );
    }
    finding
}
