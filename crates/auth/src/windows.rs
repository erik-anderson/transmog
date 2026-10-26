//! Readable NTLM, GSS/SPNEGO and Kerberos envelopes. No impersonation, decryption or validation.
use crate::{Context, Finding, MAX_DEPTH, MAX_FIELDS, Source};
use base64::{Engine as _, engine::general_purpose::STANDARD};

fn u16le(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}
fn u32le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}
fn buffer(bytes: &[u8], at: usize, minimum: usize) -> Option<&[u8]> {
    let length = usize::from(u16le(bytes, at)?);
    let offset = usize::try_from(u32le(bytes, at + 4)?).ok()?;
    if length == 0 {
        return Some(&[]);
    }
    if offset < minimum {
        return None;
    }
    bytes.get(offset..offset.checked_add(length)?)
}
fn identity(context: &mut Context, finding: &mut Finding, name: &str, bytes: &[u8], unicode: bool) {
    if unicode {
        if !bytes.len().is_multiple_of(2) {
            finding.issue(
                "malformed",
                "NTLM UTF-16LE identity has an odd byte length.",
            );
            return;
        }
        let words: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        match String::from_utf16(&words) {
            Ok(value) => context.field(
                finding,
                name,
                value,
                false,
                "Observed diagnostic identity; not authenticated. May contain personal data.",
            ),
            Err(_) => finding.issue("malformed", "NTLM identity has invalid UTF-16LE."),
        }
    } else {
        context.field(finding, name, if bytes.is_ascii() { String::from_utf8_lossy(bytes).into_owned() } else { format!("OEM bytes: {}", hex(bytes)) }, false, "OEM code page is unknown; non-ASCII bytes remain hexadecimal rather than guessing a character set.");
    }
}
fn flags(context: &mut Context, finding: &mut Finding, raw: u32) {
    let known: &[(u32, &str)] = &[
        (1, "UNICODE"),
        (2, "OEM"),
        (4, "REQUEST_TARGET"),
        (0x10, "SIGN"),
        (0x20, "SEAL"),
        (0x40, "DATAGRAM"),
        (0x80, "LM_KEY"),
        (0x200, "NTLM"),
        (0x800, "ANONYMOUS"),
        (0x1000, "OEM_DOMAIN_SUPPLIED"),
        (0x2000, "OEM_WORKSTATION_SUPPLIED"),
        (0x8000, "ALWAYS_SIGN"),
        (0x10000, "TARGET_TYPE_DOMAIN"),
        (0x20000, "TARGET_TYPE_SERVER"),
        (0x80000, "EXTENDED_SESSIONSECURITY"),
        (0x0010_0000, "IDENTIFY"),
        (0x0040_0000, "REQUEST_NON_NT_SESSION_KEY"),
        (0x0080_0000, "TARGET_INFO"),
        (0x0200_0000, "VERSION"),
        (0x2000_0000, "128"),
        (0x4000_0000, "KEY_EXCH"),
        (0x8000_0000, "56"),
    ];
    context.field(
        finding,
        "Negotiation flags",
        format!("0x{raw:08x}"),
        false,
        "Observed bits do not establish negotiated security or NTLM version.",
    );
    let mut mask = 0;
    for &(bit, name) in known {
        mask |= bit;
        if raw & bit != 0 {
            context.technical_field(finding, name, "Set", false, "Observed negotiation bit.");
        }
    }
    if raw & !mask != 0 {
        context.field(
            finding,
            "Unknown flag bits",
            format!("0x{:08x}", raw & !mask),
            false,
            "Uninterpreted extension/reserved bits.",
        );
    }
}
fn version(
    context: &mut Context,
    finding: &mut Finding,
    bytes: &[u8],
    offset: usize,
    raw_flags: u32,
) {
    if raw_flags & 0x0200_0000 == 0 {
        return;
    }
    if let Some(value) = bytes.get(offset..offset + 8) {
        context.field(
            finding,
            "Version metadata",
            format!(
                "{}.{} build {} · revision {}",
                value[0],
                value[1],
                u16::from_le_bytes([value[2], value[3]]),
                value[7]
            ),
            false,
            "Captured version metadata; does not identify an OS edition.",
        );
    } else {
        finding.issue(
            "incomplete",
            "Version flag is present but version bytes are missing.",
        );
    }
}
fn av_pairs(context: &mut Context, finding: &mut Finding, bytes: &[u8]) {
    let mut at = 0;
    let mut terminated = false;
    for _ in 0..256 {
        if !context.step() {
            context.limited();
            break;
        }
        let (Some(id), Some(length)) = (u16le(bytes, at), u16le(bytes, at + 2)) else {
            break;
        };
        at += 4;
        if id == 0 {
            terminated = length == 0 && at == bytes.len();
            break;
        }
        let Some(value) = bytes.get(at..at + usize::from(length)) else {
            finding.issue(
                "malformed",
                "NTLM target-info length exceeds the captured buffer.",
            );
            return;
        };
        at += usize::from(length);
        let name = match id {
            1 => "NetBIOS computer",
            2 => "NetBIOS domain",
            3 => "DNS computer",
            4 => "DNS domain",
            5 => "DNS tree",
            6 => "AV flags",
            7 => "Timestamp",
            8 => "Single-host data",
            9 => "Target SPN",
            10 => "Channel bindings",
            _ => "Unknown AV pair",
        };
        match id {
            1..=5 | 9 => identity(context, finding, name, value, true),
            6 if value.len() == 4 => context.field(finding, name, format!("0x{:08x}", u32le(value, 0).unwrap_or(0)), false, "Observed AV flags, including MIC-related flags; no MIC validation."),
            7 if value.len() == 8 => {
                let ticks = u64::from_le_bytes(value.try_into().expect("checked eight bytes"));
                let utc = i64::try_from(ticks / 10_000_000).ok().and_then(|s| s.checked_sub(11_644_473_600)).and_then(|s| time::OffsetDateTime::from_unix_timestamp(s).ok()).and_then(|v| v.format(&time::format_description::well_known::Rfc3339).ok()).unwrap_or_else(|| "UTC unavailable".into());
                context.field(finding, name, format!("{ticks} · {utc}"), false, "Windows FILETIME; raw 100 ns count preserved.");
            }
            6 | 7 => finding.issue("malformed", "NTLM AV pair has an invalid fixed field length."),
            _ => context.field(finding, &format!("{name} ({id})"), STANDARD.encode(value), true, "Uninterpreted AV bytes (Base64); sensitive channel-binding/host material stays masked."),
        }
    }
    if !terminated {
        finding.issue("incomplete", "NTLM target-info terminator is missing, malformed, followed by data, or beyond the AV budget.");
    }
}
fn ntlm(context: &mut Context, bytes: &[u8], source: Source) -> Finding {
    let mut finding = Finding::new("NTLM", "mechanism token", source);
    finding.verification = "Not checked".into();
    if !bytes.starts_with(b"NTLMSSP\0") {
        finding.recognition = "candidate".into();
        finding.issue("malformed", "NTLM signature is missing.");
        return finding;
    }
    let Some(kind) = u32le(bytes, 8) else {
        finding.issue("incomplete", "NTLM message type is truncated.");
        return finding;
    };
    finding.role = match kind {
        1 => "Type 1 · Negotiate",
        2 => "Type 2 · Challenge",
        3 => "Type 3 · Authenticate",
        _ => "Unknown message type",
    }
    .into();
    context.field(
        &mut finding,
        "Message type",
        kind.to_string(),
        false,
        "Authenticate and Challenge messages do not prove server acceptance.",
    );
    let min = match kind {
        1 => 16,
        2 => 32,
        3 => 52,
        _ => {
            finding.issue("unsupported", "Unknown NTLM message type.");
            return finding;
        }
    };
    if bytes.len() < min {
        finding.issue("incomplete", "NTLM fixed message structure is truncated.");
        return finding;
    }
    let raw_flags = match kind {
        1 => u32le(bytes, 12),
        2 => u32le(bytes, 20),
        _ => u32le(bytes, 60),
    }
    .unwrap_or(0);
    flags(context, &mut finding, raw_flags);
    let version_at = match kind {
        1 => 32,
        2 => 48,
        _ => 64,
    };
    version(context, &mut finding, bytes, version_at, raw_flags);
    let minimum_payload = if raw_flags & 0x0200_0000 != 0 {
        version_at + 8
    } else {
        min
    };
    let fields: &[(usize, &str, bool)] = match kind {
        1 => &[(16, "Domain", false), (24, "Workstation", false)],
        2 => &[(12, "Target name", false)],
        _ => &[
            (12, "LM response", true),
            (20, "NT response", true),
            (28, "Domain", false),
            (36, "Username", false),
            (44, "Workstation", false),
            (52, "Encrypted session key", true),
        ],
    };
    for &(at, name, secret) in fields {
        if bytes.get(at..at + 8).is_none() {
            if kind == 1 && raw_flags & 0x3000 == 0 || kind == 3 && at == 52 {
                continue;
            }
            finding.issue(
                "incomplete",
                "An NTLM security-buffer descriptor is absent.",
            );
            continue;
        }
        let Some(value) = buffer(bytes, at, minimum_payload) else {
            finding.issue(
                "malformed",
                "NTLM security-buffer offset or length is outside its payload bounds.",
            );
            continue;
        };
        if secret {
            context.field(
                &mut finding,
                &format!("{name} length"),
                value.len().to_string(),
                false,
                "Captured response/key length; not a password and not validated.",
            );
            context.technical_field(
                &mut finding,
                name,
                STANDARD.encode(value),
                true,
                "Cryptographic material (Base64); no response validation or key recovery.",
            );
            if name == "NT response"
                && value.len() >= 48
                && value.get(16..24) == Some(&[1, 1, 0, 0, 0, 0, 0, 0])
            {
                context.field(&mut finding, "NTLMv2-style response", "Observed versioned client challenge structure", false, "Identified from response bytes, not negotiation flags; no authentication is established.");
                // Four trailing reserved bytes follow the AV pairs in NTLMv2_CLIENT_CHALLENGE.
                if value.len() >= 52 {
                    av_pairs(context, &mut finding, &value[44..value.len() - 4]);
                }
            }
        } else {
            identity(
                context,
                &mut finding,
                name,
                value,
                kind != 1 && raw_flags & 1 != 0,
            );
        }
    }
    if kind == 2 {
        context.technical_field(
            &mut finding,
            "Server challenge",
            STANDARD.encode(&bytes[24..32]),
            true,
            "Sensitive challenge bytes; not validated.",
        );
        if raw_flags & 0x0080_0000 != 0 || bytes.len() >= 48 {
            if let Some(value) = buffer(bytes, 40, minimum_payload.max(48)) {
                av_pairs(context, &mut finding, value);
            } else {
                finding.issue(
                    "malformed",
                    "NTLM target-info security buffer is missing or outside the payload.",
                );
            }
        }
    }
    if kind == 3 {
        let first_payload = [12, 20, 28, 36, 44, 52]
            .into_iter()
            .filter(|at| u16le(bytes, *at).is_some_and(|n| n > 0))
            .filter_map(|at| u32le(bytes, at + 4))
            .min()
            .and_then(|v| usize::try_from(v).ok());
        let start = if raw_flags & 0x0200_0000 != 0 { 72 } else { 64 };
        if first_payload.is_some_and(|end| end >= start + 16)
            && let Some(value) = bytes.get(start..start + 16)
        {
            context.technical_field(&mut finding,"Possible MIC / optional bytes",STANDARD.encode(value),true,"Bytes occupy the optional MIC region; presence and validity are not inferred from padding alone.");
        }
    }
    finding.diagnostics.push("Observed identities and cryptographic messages are unverified; a completed handshake or successful login is not inferred.".into());
    finding
}

struct Tlv<'a> {
    tag: u8,
    bytes: &'a [u8],
    rest: &'a [u8],
}
fn tlv(bytes: &[u8]) -> Option<Tlv<'_>> {
    let tag = *bytes.first()?;
    if tag & 31 == 31 {
        return None;
    }
    let first = *bytes.get(1)?;
    let mut at = 2;
    let length = if first < 128 {
        usize::from(first)
    } else {
        let count = usize::from(first & 127);
        if count == 0 || count > 4 || bytes.get(2) == Some(&0) {
            return None;
        }
        let mut size = 0usize;
        for byte in bytes.get(at..at + count)? {
            size = size.checked_mul(256)?.checked_add(usize::from(*byte))?;
        }
        at += count;
        if size < 128 {
            return None;
        }
        size
    };
    let end = at.checked_add(length)?;
    Some(Tlv {
        tag,
        bytes: bytes.get(at..end)?,
        rest: bytes.get(end..)?,
    })
}
fn children(bytes: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut rest = bytes;
    let mut result = vec![];
    while !rest.is_empty() {
        if result.len() >= 256 {
            return None;
        }
        let item = tlv(rest)?;
        rest = item.rest;
        result.push(item);
    }
    Some(result)
}
fn validate(bytes: &[u8], depth: usize, nodes: &mut usize, context: &mut Context) -> bool {
    if depth > MAX_DEPTH || *nodes >= MAX_FIELDS || !context.step() {
        return false;
    }
    let Some(items) = children(bytes) else {
        return false;
    };
    for item in items {
        *nodes += 1;
        if *nodes > MAX_FIELDS
            || item.tag & 32 != 0 && !validate(item.bytes, depth + 1, nodes, context)
        {
            return false;
        }
    }
    true
}
fn oid(bytes: &[u8]) -> Option<String> {
    let mut arcs = vec![];
    let mut number = 0u64;
    let mut pending = false;
    for byte in bytes {
        number = number
            .checked_mul(128)?
            .checked_add(u64::from(byte & 127))?;
        pending = byte & 128 != 0;
        if !pending {
            if arcs.is_empty() {
                let first = (number / 40).min(2);
                arcs.push(first);
                arcs.push(number - first * 40);
            } else {
                arcs.push(number);
            }
            number = 0;
        }
    }
    if pending || arcs.is_empty() {
        return None;
    }
    Some(
        arcs.iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("."),
    )
}
fn mechanism(value: &str) -> &'static str {
    match value {
        "1.3.6.1.5.5.2" => "SPNEGO",
        "1.2.840.113554.1.2.2" | "1.2.840.48018.1.2.2" => "Kerberos V5",
        "1.3.6.1.4.1.311.2.2.10" => "NTLM",
        _ => "Unknown mechanism",
    }
}
fn int(bytes: &[u8]) -> Option<i64> {
    let item = tlv(bytes)?;
    if item.tag != 2 && item.tag != 10
        || !item.rest.is_empty()
        || item.bytes.is_empty()
        || item.bytes.len() > 8
    {
        return None;
    }
    let mut value = if item.bytes[0] & 128 != 0 { -1i64 } else { 0 };
    for b in item.bytes {
        value = (value << 8) | i64::from(*b);
    }
    Some(value)
}
fn context_field<'a>(items: &'a [Tlv<'a>], tag: u8) -> Option<&'a [u8]> {
    let mut matches = items.iter().filter(|v| v.tag == tag);
    let value = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(value.bytes)
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
        && [0x1b, 0x1c, 0x0c, 0x18].contains(&item.tag)
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
    let Some(sequence) = tlv(bytes).filter(|v| v.tag == 0x30 && v.rest.is_empty()) else {
        finding.issue("malformed", "Invalid Kerberos encrypted-data structure.");
        return;
    };
    let Some(items) = children(sequence.bytes) else {
        return;
    };
    for (tag, label) in [(0xa0, "encryption type"), (0xa1, "key version")] {
        if let Some(value) = context_field(&items, tag) {
            scalar(context, finding, &format!("{name} · {label}"), value, false);
        }
    }
    if let Some(value) = context_field(&items, 0xa2)
        .and_then(tlv)
        .filter(|v| v.tag == 4)
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
    let Some(sequence) = tlv(bytes).filter(|v| v.tag == 0x30) else {
        return;
    };
    let Some(items) = children(sequence.bytes) else {
        return;
    };
    if let Some(kind) = context_field(&items, 0xa0) {
        scalar(context, finding, &format!("{name} type"), kind, false);
    }
    if let Some(sequence) = context_field(&items, 0xa1)
        .and_then(tlv)
        .filter(|v| v.tag == 0x30)
        && let Some(parts) = children(sequence.bytes)
    {
        for (index, part) in parts.iter().enumerate() {
            if part.tag == 0x1b {
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
fn kerberos(context: &mut Context, bytes: &[u8], source: Source) -> Finding {
    let mut finding = Finding::new("Kerberos V5", "mechanism token", source);
    finding.verification = "Not checked".into();
    let Some(outer) = tlv(bytes).filter(|v| v.rest.is_empty()) else {
        finding.issue("malformed", "Invalid Kerberos application envelope.");
        return finding;
    };
    finding.role = match outer.tag {
        0x6e => "AP-REQ",
        0x6f => "AP-REP",
        0x7e => "KRB-ERROR",
        _ => "Unknown Kerberos application message",
    }
    .into();
    let Some(sequence) = tlv(outer.bytes).filter(|v| v.tag == 0x30 && v.rest.is_empty()) else {
        finding.issue("malformed", "Kerberos application body is not a SEQUENCE.");
        return finding;
    };
    let Some(items) = children(sequence.bytes) else {
        finding.issue("malformed", "Invalid Kerberos fields.");
        return finding;
    };
    if context_field(&items, 0xa0).and_then(int) != Some(5)
        || context_field(&items, 0xa1).and_then(int) != Some(i64::from(outer.tag & 31))
    {
        finding.issue("malformed", "Kerberos protocol version/message type is missing, duplicated, mistyped or inconsistent with the application tag.");
    }
    for (tag, name) in [(0xa0, "Protocol version"), (0xa1, "Message type")] {
        if let Some(value) = context_field(&items, tag) {
            scalar(context, &mut finding, name, value, false);
        }
    }
    match outer.tag {
        0x6e => {
            if let Some(value) = context_field(&items, 0xa2)
                .and_then(tlv)
                .filter(|v| v.tag == 3)
            {
                context.field(
                    &mut finding,
                    "AP options",
                    hex(value.bytes),
                    false,
                    "DER BIT STRING including unused-bit count; options are unverified.",
                );
                if value.bytes.len() != 5 || value.bytes[0] != 0 {
                    finding.issue(
                        "malformed",
                        "Kerberos AP options do not have the supported 32-bit BIT STRING shape.",
                    );
                } else {
                    context.field(
                        &mut finding,
                        "Use session key",
                        (value.bytes[1] & 0x40 != 0).to_string(),
                        false,
                        "Observed AP option; no encrypted ticket/session key is recovered.",
                    );
                    context.field(
                        &mut finding,
                        "Mutual authentication required",
                        (value.bytes[1] & 0x20 != 0).to_string(),
                        false,
                        "Requested option, not proof that mutual authentication completed.",
                    );
                }
            }
            if let Some(ticket) = context_field(&items, 0xa3)
                .and_then(tlv)
                .filter(|v| v.tag == 0x61)
            {
                if let Some(sequence) = tlv(ticket.bytes).filter(|v| v.tag == 0x30)
                    && let Some(ticket) = children(sequence.bytes)
                {
                    if let Some(value) = context_field(&ticket, 0xa0) {
                        scalar(context, &mut finding, "Ticket version", value, false);
                    }
                    if let Some(value) = context_field(&ticket, 0xa1) {
                        scalar(context, &mut finding, "Ticket realm", value, false);
                    }
                    if let Some(value) = context_field(&ticket, 0xa2) {
                        principal(context, &mut finding, "Service principal", value);
                    }
                    if let Some(value) = context_field(&ticket, 0xa3) {
                        encrypted(context, &mut finding, "Ticket", value);
                    }
                }
            } else {
                finding.issue("incomplete", "AP-REQ ticket is absent or malformed.");
            }
            if let Some(value) = context_field(&items, 0xa4) {
                encrypted(context, &mut finding, "Authenticator", value);
            } else {
                finding.issue("incomplete", "AP-REQ authenticator is absent.");
            }
        }
        0x6f => {
            if let Some(value) = context_field(&items, 0xa2) {
                encrypted(context, &mut finding, "Reply", value);
            } else {
                finding.issue("incomplete", "AP-REP encrypted part is absent.");
            }
        }
        0x7e => {
            for (tag, name) in [
                (0xa2, "Client time"),
                (0xa4, "Server time"),
                (0xa6, "Error code"),
                (0xa7, "Client realm"),
                (0xa9, "Realm"),
                (0xab, "Error text"),
            ] {
                if let Some(value) = context_field(&items, tag) {
                    scalar(context, &mut finding, name, value, tag == 0xab);
                }
            }
            if let Some(value) = context_field(&items, 0xaa) {
                principal(context, &mut finding, "Service principal", value);
            }
        }
        _ => finding.issue("unsupported", "Kerberos application type is not supported."),
    }
    finding.diagnostics.push("Encrypted ticket/authenticator/reply contents do not reveal client identity, session keys, ticket lifetime or authorization data. Signatures and server acceptance are not checked.".into());
    finding
}
fn negotiate(context: &mut Context, bytes: &[u8], mut source: Source, depth: usize) -> Finding {
    if bytes.starts_with(b"NTLMSSP\0") {
        return ntlm(context, bytes, source);
    }
    let mut finding = Finding::new("GSS-API / SPNEGO", "negotiation token", source.clone());
    finding.verification = "Not checked".into();
    if depth > MAX_DEPTH || !context.step() {
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
    if outer.tag == 0x60 {
        let Some(mech) = tlv(outer.bytes).filter(|v| v.tag == 6) else {
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
            let payload = if mech.rest.starts_with(&[1, 0])
                || mech.rest.starts_with(&[2, 0])
                || mech.rest.starts_with(&[3, 0])
            {
                &mech.rest[2..]
            } else {
                mech.rest
            };
            let mut nodes = 0;
            if validate(payload, 0, &mut nodes, context) {
                finding.children.push(kerberos(context, payload, source));
            } else {
                finding.issue(
                    "malformed",
                    "Kerberos DER lengths, depth or nodes are invalid.",
                );
            }
        } else if mechanism(&value) == "SPNEGO" {
            finding
                .children
                .push(negotiate(context, mech.rest, source, depth + 1));
        } else if mechanism(&value) == "NTLM" {
            finding.children.push(ntlm(context, mech.rest, source));
        } else {
            finding.issue("unsupported", "Unknown GSS mechanism remains opaque.");
        }
        return finding;
    }
    let mut nodes = 0;
    if !validate(bytes, 0, &mut nodes, context) {
        finding.issue(
            "malformed",
            "ASN.1 lengths, depth or node budget is invalid.",
        );
        return finding;
    }
    if [0x6e, 0x6f, 0x7e].contains(&outer.tag) {
        return kerberos(context, bytes, source);
    }
    if outer.tag != 0xa0 && outer.tag != 0xa1 {
        finding.issue("unsupported", "Unknown Negotiate token variant.");
        return finding;
    }
    finding.format = "SPNEGO".into();
    finding.role = if outer.tag == 0xa0 {
        "NegTokenInit"
    } else {
        "NegTokenResp"
    }
    .into();
    let Some(sequence) = tlv(outer.bytes).filter(|v| v.tag == 0x30) else {
        finding.issue("malformed", "SPNEGO choice does not contain a sequence.");
        return finding;
    };
    let Some(items) = children(sequence.bytes) else {
        return finding;
    };
    if outer.tag == 0xa0 {
        if let Some(sequence) = context_field(&items, 0xa0)
            .and_then(tlv)
            .filter(|v| v.tag == 0x30)
            && let Some(mechs) = children(sequence.bytes)
        {
            for mech in mechs {
                if mech.tag == 6
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
        if let Some(flags) = context_field(&items, 0xa1)
            .and_then(tlv)
            .filter(|v| v.tag == 3)
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
        if let Some(value) = context_field(&items, 0xa0).and_then(int) {
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
        if let Some(value) = context_field(&items, 0xa1)
            .and_then(tlv)
            .filter(|v| v.tag == 6)
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
    if let Some(value) = context_field(&items, 0xa2)
        .and_then(tlv)
        .filter(|v| v.tag == 4)
    {
        source
            .transforms
            .push("SPNEGO mechanism OCTET STRING".into());
        finding
            .children
            .push(negotiate(context, value.bytes, source, depth + 1));
    }
    if let Some(value) = context_field(&items, 0xa3)
        .and_then(tlv)
        .filter(|v| v.tag == 4)
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
pub(crate) fn decode(
    context: &mut Context,
    bytes: &[u8],
    source: Source,
    explicit_ntlm: bool,
) -> Finding {
    let mut finding = if explicit_ntlm {
        ntlm(context, bytes, source)
    } else {
        negotiate(context, bytes, source, 0)
    };
    if context.charge(bytes.len()) {
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
    context.technical_field(&mut finding, "Decoded mechanism bytes (hex prefix)", hex(&bytes[..bytes.len().min(4096)]), true, "At most the first 4 KiB of decoded mechanism bytes; full encoded material remains in the enclosing credential/source. Includes sensitive cryptographic data.");
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
