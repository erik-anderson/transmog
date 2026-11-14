//! Exposed Kerberos V5 metadata; ticket and authenticator contents stay encrypted.
use super::asn1::{TAG_NUMBER_MASK, tag};
use super::{
    asn1::{children, context_field, int, tlv},
    hex,
};
use crate::{Context, Finding, Source};

const PROTOCOL_VERSION: i64 = 5;
const AP_OPTIONS_BYTES: usize = 1 + size_of::<u32>();
const USE_SESSION_KEY_FLAG: u8 = 0x40;
const MUTUAL_AUTHENTICATION_FLAG: u8 = 0x20;

// Context-specific tags are scoped to their ASN.1 structure.
mod encrypted_field {
    pub const ENCRYPTION_TYPE: u8 = 0xa0;
    pub const KEY_VERSION: u8 = 0xa1;
    pub const CIPHERTEXT: u8 = 0xa2;
}
mod principal_field {
    pub const NAME_TYPE: u8 = 0xa0;
    pub const NAME_PARTS: u8 = 0xa1;
}
mod message_field {
    pub const PROTOCOL_VERSION: u8 = 0xa0;
    pub const MESSAGE_TYPE: u8 = 0xa1;
    pub const AP_OPTIONS: u8 = 0xa2;
    pub const TICKET: u8 = 0xa3;
    pub const AUTHENTICATOR: u8 = 0xa4;
    pub const REPLY_ENCRYPTED_PART: u8 = 0xa2;
}
mod ticket_field {
    pub const VERSION: u8 = 0xa0;
    pub const REALM: u8 = 0xa1;
    pub const SERVICE_PRINCIPAL: u8 = 0xa2;
    pub const ENCRYPTED_PART: u8 = 0xa3;
}
mod error_field {
    pub const CLIENT_TIME: u8 = 0xa2;
    pub const SERVER_TIME: u8 = 0xa4;
    pub const CODE: u8 = 0xa6;
    pub const CLIENT_REALM: u8 = 0xa7;
    pub const REALM: u8 = 0xa9;
    pub const SERVICE_PRINCIPAL: u8 = 0xaa;
    pub const TEXT: u8 = 0xab;
}

fn scalar(context: &mut Context, finding: &mut Finding, name: &str, bytes: &[u8], secret: bool) {
    if let Some(value) = int(bytes) {
        context.field(
            finding,
            name,
            value.to_string(),
            secret,
            "Unverified ASN.1 integer.",
        );
    } else if let Some(item) = tlv(bytes)
        && item.rest.is_empty()
        && [
            tag::GENERAL_STRING,
            tag::UNIVERSAL_STRING,
            tag::UTF8_STRING,
            tag::GENERALIZED_TIME,
        ]
        .contains(&item.tag)
    {
        context.field(
            finding,
            name,
            String::from_utf8_lossy(item.bytes),
            secret,
            "Unverified exposed Kerberos text.",
        );
    }
}
fn encrypted(context: &mut Context, finding: &mut Finding, name: &str, bytes: &[u8]) {
    let Some(sequence) = tlv(bytes).filter(|v| v.tag == tag::SEQUENCE && v.rest.is_empty()) else {
        finding.issue("malformed", "Invalid Kerberos encrypted-data structure.");
        return;
    };
    let Some(items) = children(sequence.bytes) else {
        return;
    };
    for (tag, label) in [
        (encrypted_field::ENCRYPTION_TYPE, "encryption type"),
        (encrypted_field::KEY_VERSION, "key version"),
    ] {
        if let Some(value) = context_field(&items, tag) {
            scalar(context, finding, &format!("{name} · {label}"), value, false);
        }
    }
    if let Some(value) = context_field(&items, encrypted_field::CIPHERTEXT)
        .and_then(tlv)
        .filter(|v| v.tag == tag::OCTET_STRING)
    {
        context.field(
            finding,
            &format!("{name} · ciphertext length"),
            value.bytes.len().to_string(),
            false,
            "Encrypted content is unreadable without keys. No decryption is attempted.",
        );
        if finding.status == "decoded" {
            finding.status = "encrypted".into();
        }
    } else {
        finding.issue(
            "malformed",
            "Kerberos encrypted data lacks its ciphertext OCTET STRING.",
        );
    }
}
fn principal(context: &mut Context, finding: &mut Finding, name: &str, bytes: &[u8]) {
    let Some(sequence) = tlv(bytes).filter(|v| v.tag == tag::SEQUENCE) else {
        return;
    };
    let Some(items) = children(sequence.bytes) else {
        return;
    };
    if let Some(kind) = context_field(&items, principal_field::NAME_TYPE) {
        scalar(context, finding, &format!("{name} type"), kind, false);
    }
    if let Some(sequence) = context_field(&items, principal_field::NAME_PARTS)
        .and_then(tlv)
        .filter(|v| v.tag == tag::SEQUENCE)
        && let Some(parts) = children(sequence.bytes)
    {
        for (index, part) in parts.iter().enumerate() {
            if part.tag == tag::GENERAL_STRING {
                context.field(
                    finding,
                    &format!("{name}[{index}]"),
                    String::from_utf8_lossy(part.bytes),
                    false,
                    "Outer service principal; not the encrypted ticket's client identity.",
                );
            }
        }
    }
}
pub(super) fn decode(context: &mut Context, bytes: &[u8], source: Source) -> Finding {
    let mut finding = Finding::new("Kerberos V5", "mechanism token", source);
    finding.verification = "Not checked".into();
    let Some(outer) = tlv(bytes).filter(|v| v.rest.is_empty()) else {
        finding.issue("malformed", "Invalid Kerberos application envelope.");
        return finding;
    };
    finding.role = match outer.tag {
        tag::KERBEROS_AP_REQUEST => "AP-REQ",
        tag::KERBEROS_AP_REPLY => "AP-REP",
        tag::KERBEROS_ERROR => "KRB-ERROR",
        _ => "Unknown Kerberos application message",
    }
    .into();
    let Some(sequence) = tlv(outer.bytes).filter(|v| v.tag == tag::SEQUENCE && v.rest.is_empty())
    else {
        finding.issue("malformed", "Kerberos application body is not a SEQUENCE.");
        return finding;
    };
    let Some(items) = children(sequence.bytes) else {
        finding.issue("malformed", "Invalid Kerberos fields.");
        return finding;
    };
    if context_field(&items, message_field::PROTOCOL_VERSION).and_then(int)
        != Some(PROTOCOL_VERSION)
        || context_field(&items, message_field::MESSAGE_TYPE).and_then(int)
            != Some(i64::from(outer.tag & TAG_NUMBER_MASK))
    {
        finding.issue("malformed", "Kerberos protocol version/message type is missing, duplicated, mistyped or inconsistent with the application tag.");
    }
    for (tag, name) in [
        (message_field::PROTOCOL_VERSION, "Protocol version"),
        (message_field::MESSAGE_TYPE, "Message type"),
    ] {
        if let Some(value) = context_field(&items, tag) {
            scalar(context, &mut finding, name, value, false);
        }
    }
    match outer.tag {
        tag::KERBEROS_AP_REQUEST => {
            if let Some(value) = context_field(&items, message_field::AP_OPTIONS)
                .and_then(tlv)
                .filter(|v| v.tag == tag::BIT_STRING)
            {
                context.field(
                    &mut finding,
                    "AP options",
                    hex(value.bytes),
                    false,
                    "DER BIT STRING including unused-bit count; options are unverified.",
                );
                if value.bytes.len() != AP_OPTIONS_BYTES || value.bytes[0] != 0 {
                    finding.issue(
                        "malformed",
                        "Kerberos AP options do not have the supported 32-bit BIT STRING shape.",
                    );
                } else {
                    context.field(
                        &mut finding,
                        "Use session key",
                        (value.bytes[1] & USE_SESSION_KEY_FLAG != 0).to_string(),
                        false,
                        "Observed AP option; no encrypted ticket/session key is recovered.",
                    );
                    context.field(
                        &mut finding,
                        "Mutual authentication required",
                        (value.bytes[1] & MUTUAL_AUTHENTICATION_FLAG != 0).to_string(),
                        false,
                        "Requested option, not proof that mutual authentication completed.",
                    );
                }
            }
            if let Some(ticket) = context_field(&items, message_field::TICKET)
                .and_then(tlv)
                .filter(|v| v.tag == tag::KERBEROS_TICKET)
            {
                if let Some(sequence) = tlv(ticket.bytes).filter(|v| v.tag == tag::SEQUENCE)
                    && let Some(ticket) = children(sequence.bytes)
                {
                    if let Some(value) = context_field(&ticket, ticket_field::VERSION) {
                        scalar(context, &mut finding, "Ticket version", value, false);
                    }
                    if let Some(value) = context_field(&ticket, ticket_field::REALM) {
                        scalar(context, &mut finding, "Ticket realm", value, false);
                    }
                    if let Some(value) = context_field(&ticket, ticket_field::SERVICE_PRINCIPAL) {
                        principal(context, &mut finding, "Service principal", value);
                    }
                    if let Some(value) = context_field(&ticket, ticket_field::ENCRYPTED_PART) {
                        encrypted(context, &mut finding, "Ticket", value);
                    }
                }
            } else {
                finding.issue("incomplete", "AP-REQ ticket is absent or malformed.");
            }
            if let Some(value) = context_field(&items, message_field::AUTHENTICATOR) {
                encrypted(context, &mut finding, "Authenticator", value);
            } else {
                finding.issue("incomplete", "AP-REQ authenticator is absent.");
            }
        }
        tag::KERBEROS_AP_REPLY => {
            if let Some(value) = context_field(&items, message_field::REPLY_ENCRYPTED_PART) {
                encrypted(context, &mut finding, "Reply", value);
            } else {
                finding.issue("incomplete", "AP-REP encrypted part is absent.");
            }
        }
        tag::KERBEROS_ERROR => {
            for (tag, name) in [
                (error_field::CLIENT_TIME, "Client time"),
                (error_field::SERVER_TIME, "Server time"),
                (error_field::CODE, "Error code"),
                (error_field::CLIENT_REALM, "Client realm"),
                (error_field::REALM, "Realm"),
                (error_field::TEXT, "Error text"),
            ] {
                if let Some(value) = context_field(&items, tag) {
                    scalar(context, &mut finding, name, value, tag == error_field::TEXT);
                }
            }
            if let Some(value) = context_field(&items, error_field::SERVICE_PRINCIPAL) {
                principal(context, &mut finding, "Service principal", value);
            }
        }
        _ => finding.issue("unsupported", "Kerberos application type is not supported."),
    }
    finding.diagnostics.push("Encrypted ticket/authenticator/reply contents do not reveal client identity, session keys, ticket lifetime or authorization data. Signatures and server acceptance are not checked.".into());
    finding
}
