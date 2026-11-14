//! NTLM message layouts, security buffers and readable identities.
use super::hex;
use crate::limits::MAX_NTLM_AV_PAIRS;
use crate::{Context, Finding, Source};
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub(super) const SIGNATURE: &[u8] = b"NTLMSSP\0";
const MESSAGE_TYPE_OFFSET: usize = SIGNATURE.len();
const NEGOTIATE_MESSAGE: u32 = 1;
const CHALLENGE_MESSAGE: u32 = 2;
const AUTHENTICATE_MESSAGE: u32 = 3;
const UNICODE_FLAG: u32 = 0x0000_0001;
const DOMAIN_SUPPLIED_FLAG: u32 = 0x0000_1000;
const WORKSTATION_SUPPLIED_FLAG: u32 = 0x0000_2000;
const TARGET_INFO_FLAG: u32 = 0x0080_0000;
const VERSION_FLAG: u32 = 0x0200_0000;
const SECURITY_BUFFER_BYTES: usize = 8;
const SECURITY_BUFFER_DATA_OFFSET: usize = 4;
const SESSION_KEY_BUFFER_OFFSET: usize = 52;
const VERSION_BYTES: usize = 8;
const MIC_BYTES: usize = 16;
const CHALLENGE_OFFSET: usize = 24;
const CHALLENGE_BYTES: usize = 8;
const TARGET_INFO_BUFFER_OFFSET: usize = 40;
const TARGET_INFO_HEADER_BYTES: usize = TARGET_INFO_BUFFER_OFFSET + SECURITY_BUFFER_BYTES;
const V2_SIGNATURE_OFFSET: usize = 16;
const V2_SIGNATURE: &[u8] = &[1, 1, 0, 0, 0, 0, 0, 0];
const V2_MINIMUM_RESPONSE_BYTES: usize = 48;
const V2_TARGET_INFO_OFFSET: usize = 44;
const V2_TRAILER_BYTES: usize = 4;
const V2_MINIMUM_TARGET_INFO_BYTES: usize = 52;
const AV_HEADER_BYTES: usize = 4;
const AV_FLAGS: u16 = 6;
const AV_TIMESTAMP: u16 = 7;
const AV_TARGET_SPN: u16 = 9;
const FILETIME_TICKS_PER_SECOND: u64 = 10_000_000;
const FILETIME_UNIX_EPOCH_SECONDS: i64 = 11_644_473_600;

struct MessageLayout {
    role: &'static str,
    minimum_bytes: usize,
    flags_offset: usize,
    version_offset: usize,
    // Offset, name and sensitivity of each security-buffer descriptor.
    fields: &'static [(usize, &'static str, bool)],
}

const NEGOTIATE_LAYOUT: MessageLayout = MessageLayout {
    role: "Type 1 · Negotiate",
    minimum_bytes: 16,
    flags_offset: 12,
    version_offset: 32,
    fields: &[(16, "Domain", false), (24, "Workstation", false)],
};
const CHALLENGE_LAYOUT: MessageLayout = MessageLayout {
    role: "Type 2 · Challenge",
    minimum_bytes: 32,
    flags_offset: 20,
    version_offset: 48,
    fields: &[(12, "Target name", false)],
};
const AUTHENTICATE_LAYOUT: MessageLayout = MessageLayout {
    role: "Type 3 · Authenticate",
    minimum_bytes: 52,
    flags_offset: 60,
    version_offset: 64,
    fields: &[
        (12, "LM response", true),
        (20, "NT response", true),
        (28, "Domain", false),
        (36, "Username", false),
        (44, "Workstation", false),
        (SESSION_KEY_BUFFER_OFFSET, "Encrypted session key", true),
    ],
};

fn u16le(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(at..at + size_of::<u16>())?.try_into().ok()?,
    ))
}
fn u32le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at + size_of::<u32>())?.try_into().ok()?,
    ))
}
fn buffer(bytes: &[u8], at: usize, minimum: usize) -> Option<&[u8]> {
    let length = usize::from(u16le(bytes, at)?);
    let offset = usize::try_from(u32le(bytes, at + SECURITY_BUFFER_DATA_OFFSET)?).ok()?;
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
        (UNICODE_FLAG, "UNICODE"),
        (2, "OEM"),
        (4, "REQUEST_TARGET"),
        (0x10, "SIGN"),
        (0x20, "SEAL"),
        (0x40, "DATAGRAM"),
        (0x80, "LM_KEY"),
        (0x200, "NTLM"),
        (0x800, "ANONYMOUS"),
        (DOMAIN_SUPPLIED_FLAG, "OEM_DOMAIN_SUPPLIED"),
        (WORKSTATION_SUPPLIED_FLAG, "OEM_WORKSTATION_SUPPLIED"),
        (0x8000, "ALWAYS_SIGN"),
        (0x10000, "TARGET_TYPE_DOMAIN"),
        (0x20000, "TARGET_TYPE_SERVER"),
        (0x80000, "EXTENDED_SESSIONSECURITY"),
        (0x0010_0000, "IDENTIFY"),
        (0x0040_0000, "REQUEST_NON_NT_SESSION_KEY"),
        (TARGET_INFO_FLAG, "TARGET_INFO"),
        (VERSION_FLAG, "VERSION"),
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
    if raw_flags & VERSION_FLAG == 0 {
        return;
    }
    if let Some([major, minor, build_low, build_high, _, _, _, revision]) =
        bytes.get(offset..offset + VERSION_BYTES)
    {
        context.field(
            finding,
            "Version metadata",
            format!(
                "{}.{} build {} · revision {}",
                major,
                minor,
                u16::from_le_bytes([*build_low, *build_high]),
                revision
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
fn filetime_utc(ticks: u64) -> String {
    i64::try_from(ticks / FILETIME_TICKS_PER_SECOND)
        .ok()
        .and_then(|seconds| seconds.checked_sub(FILETIME_UNIX_EPOCH_SECONDS))
        .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "UTC unavailable".into())
}

fn av_pairs(context: &mut Context, finding: &mut Finding, bytes: &[u8]) {
    let mut at = 0;
    let mut terminated = false;
    for _ in 0..MAX_NTLM_AV_PAIRS {
        if context.is_canceled() {
            context.limited();
            break;
        }
        let (Some(id), Some(length)) = (u16le(bytes, at), u16le(bytes, at + 2)) else {
            break;
        };
        at += AV_HEADER_BYTES;
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
            AV_FLAGS => "AV flags",
            AV_TIMESTAMP => "Timestamp",
            8 => "Single-host data",
            AV_TARGET_SPN => "Target SPN",
            10 => "Channel bindings",
            _ => "Unknown AV pair",
        };
        match id {
            1..=5 | AV_TARGET_SPN => identity(context, finding, name, value, true),
            AV_FLAGS if value.len() == size_of::<u32>() => context.field(finding, name, format!("0x{:08x}", u32le(value, 0).unwrap_or(0)), false, "Observed AV flags, including MIC-related flags; no MIC validation."),
            AV_TIMESTAMP if value.len() == size_of::<u64>() => {
                let ticks = u64::from_le_bytes(value.try_into().expect("checked eight bytes"));
                let utc = filetime_utc(ticks);
                context.field(finding, name, format!("{ticks} · {utc}"), false, "Windows FILETIME; raw 100 ns count preserved.");
            }
            AV_FLAGS | AV_TIMESTAMP => finding.issue("malformed", "NTLM AV pair has an invalid fixed field length."),
            _ => context.field(finding, &format!("{name} ({id})"), STANDARD.encode(value), true, "Uninterpreted AV bytes (Base64); sensitive channel-binding/host material stays masked."),
        }
    }
    if !terminated {
        finding.issue("incomplete", "NTLM target-info terminator is missing, malformed, followed by data, or beyond the AV budget.");
    }
}
pub(super) fn decode(context: &mut Context, bytes: &[u8], source: Source) -> Finding {
    let mut finding = Finding::new("NTLM", "mechanism token", source);
    finding.verification = "Not checked".into();
    if !bytes.starts_with(SIGNATURE) {
        finding.recognition = "candidate".into();
        finding.issue("malformed", "NTLM signature is missing.");
        return finding;
    }
    let Some(kind) = u32le(bytes, MESSAGE_TYPE_OFFSET) else {
        finding.issue("incomplete", "NTLM message type is truncated.");
        return finding;
    };
    let layout = match kind {
        NEGOTIATE_MESSAGE => Some(&NEGOTIATE_LAYOUT),
        CHALLENGE_MESSAGE => Some(&CHALLENGE_LAYOUT),
        AUTHENTICATE_MESSAGE => Some(&AUTHENTICATE_LAYOUT),
        _ => None,
    };
    finding.role = layout
        .map_or("Unknown message type", |layout| layout.role)
        .into();
    context.field(
        &mut finding,
        "Message type",
        kind.to_string(),
        false,
        "Authenticate and Challenge messages do not prove server acceptance.",
    );
    let Some(layout) = layout else {
        finding.issue("unsupported", "Unknown NTLM message type.");
        return finding;
    };
    let min = layout.minimum_bytes;
    if bytes.len() < min {
        finding.issue("incomplete", "NTLM fixed message structure is truncated.");
        return finding;
    }
    let raw_flags = u32le(bytes, layout.flags_offset).unwrap_or(0);
    flags(context, &mut finding, raw_flags);
    let version_at = layout.version_offset;
    version(context, &mut finding, bytes, version_at, raw_flags);
    let minimum_payload = if raw_flags & VERSION_FLAG != 0 {
        version_at + VERSION_BYTES
    } else {
        min
    };
    let fields = layout.fields;
    for &(at, name, secret) in fields {
        if bytes.get(at..at + SECURITY_BUFFER_BYTES).is_none() {
            if kind == NEGOTIATE_MESSAGE
                && raw_flags & (DOMAIN_SUPPLIED_FLAG | WORKSTATION_SUPPLIED_FLAG) == 0
                || kind == AUTHENTICATE_MESSAGE && at == SESSION_KEY_BUFFER_OFFSET
            {
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
                && value.len() >= V2_MINIMUM_RESPONSE_BYTES
                && value.get(V2_SIGNATURE_OFFSET..V2_SIGNATURE_OFFSET + V2_SIGNATURE.len())
                    == Some(V2_SIGNATURE)
            {
                context.field(&mut finding, "NTLMv2-style response", "Observed versioned client challenge structure", false, "Identified from response bytes, not negotiation flags; no authentication is established.");
                // Four trailing reserved bytes follow the AV pairs in NTLMv2_CLIENT_CHALLENGE.
                if value.len() >= V2_MINIMUM_TARGET_INFO_BYTES {
                    av_pairs(
                        context,
                        &mut finding,
                        &value[V2_TARGET_INFO_OFFSET..value.len() - V2_TRAILER_BYTES],
                    );
                }
            }
        } else {
            identity(
                context,
                &mut finding,
                name,
                value,
                kind != NEGOTIATE_MESSAGE && raw_flags & UNICODE_FLAG != 0,
            );
        }
    }
    if kind == CHALLENGE_MESSAGE {
        context.technical_field(
            &mut finding,
            "Server challenge",
            STANDARD.encode(&bytes[CHALLENGE_OFFSET..CHALLENGE_OFFSET + CHALLENGE_BYTES]),
            true,
            "Sensitive challenge bytes; not validated.",
        );
        if raw_flags & TARGET_INFO_FLAG != 0 || bytes.len() >= TARGET_INFO_HEADER_BYTES {
            if let Some(value) = buffer(
                bytes,
                TARGET_INFO_BUFFER_OFFSET,
                minimum_payload.max(TARGET_INFO_HEADER_BYTES),
            ) {
                av_pairs(context, &mut finding, value);
            } else {
                finding.issue(
                    "malformed",
                    "NTLM target-info security buffer is missing or outside the payload.",
                );
            }
        }
    }
    if kind == AUTHENTICATE_MESSAGE {
        let first_payload = layout
            .fields
            .iter()
            .map(|(offset, _, _)| *offset)
            .filter(|at| u16le(bytes, *at).is_some_and(|n| n > 0))
            .filter_map(|at| u32le(bytes, at + SECURITY_BUFFER_DATA_OFFSET))
            .min()
            .and_then(|v| usize::try_from(v).ok());
        let start = layout.version_offset
            + if raw_flags & VERSION_FLAG != 0 {
                VERSION_BYTES
            } else {
                0
            };
        if first_payload.is_some_and(|end| end >= start + MIC_BYTES)
            && let Some(value) = bytes.get(start..start + MIC_BYTES)
        {
            context.technical_field(&mut finding,"Possible MIC / optional bytes",STANDARD.encode(value),true,"Bytes occupy the optional MIC region; presence and validity are not inferred from padding alone.");
        }
    }
    finding.diagnostics.push("Observed identities and cryptographic messages are unverified; a completed handshake or successful login is not inferred.".into());
    finding
}
