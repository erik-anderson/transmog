//! Read-only interpretation of the exact server ALPS value, independent of policy.
use super::{CertificateConnection, CertificateField, field};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::fmt::Write as _;
use transmog_core::{Http2AlpsEvidence, performance::TransportObservation};

const MAX_FIELDS: usize = 128;

/// Human-readable server ALPS, with complete original bytes for the Hex viewer.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlpsContents {
    /// Exact server value size, including the negotiated protocol's frame headers.
    pub peer_bytes: usize,
    /// Frame and parameter order is retained, including duplicate settings.
    pub fields: Vec<CertificateField>,
    /// Parse failures and limits on the readable projection.
    pub notes: Vec<String>,
    /// Complete original bytes, without truncation or normalization.
    pub payload_base64: String,
}

pub(super) fn organize(connection: &mut CertificateConnection, transport: &TransportObservation) {
    let tls = transport.tls.as_ref();
    let h3 = transport.alpn.as_deref() == Some("h3")
        || tls
            .is_some_and(|tls| tls.http3_alps.is_some() || tls.details.contains_key("HTTP/3 ALPS"));
    let label = if h3 { "HTTP/3 ALPS" } else { "HTTP/2 ALPS" };
    let recorded = tls.and_then(|tls| tls.details.get(label));
    let actual = tls.and_then(|tls| {
        if h3 {
            tls.http3_alps.as_ref()
        } else {
            tls.http2_alps.as_ref()
        }
    });
    let summary = actual.map(Http2AlpsEvidence::summary);
    connection.fields.push(field(
        label,
        summary
            .as_deref()
            .or(recorded.map(String::as_str))
            .unwrap_or("Unavailable"),
    ));
    if let Some(actual) = actual {
        if recorded.is_some_and(|recorded| Some(recorded.as_str()) != summary.as_deref()) {
            connection
                .diagnostic_fields
                .push(field(label, recorded.unwrap()));
            connection.errors.push("Recorded ALPS status values disagree; additional metadata retains the other value.".into());
        }
        if actual.negotiated {
            connection.alps = Some(if h3 {
                decode_h3(&actual.peer_settings)
            } else {
                decode(&actual.peer_settings)
            });
        }
    } else if recorded.is_some() {
        connection.diagnostic_fields.push(field(
            "ALPS contents",
            "Original server bytes were not recorded in this capture.",
        ));
    }
}

impl AlpsContents {
    fn add(&mut self, label: &str, value: impl ToString) {
        let mut item = field(label, value);
        if item.display_value.chars().count() > 2048 {
            item.display_value = item.display_value.chars().take(2048).collect::<String>() + "…";
            item.display_note
                .push_str(" Display shortened; complete original bytes are available in Hex.");
        }
        self.fields.push(item);
    }
    fn warn(&mut self, offset: usize, message: &str) {
        self.notes.push(format!(
            "At byte {offset}: {message}. Original bytes are preserved in Hex."
        ));
    }
    fn full(&mut self) -> bool {
        if self.fields.len() < MAX_FIELDS {
            return false;
        }
        self.limited();
        true
    }
    fn limited(&mut self) {
        let note =
            "Readable view limited to 128 fields; complete original bytes are available in Hex.";
        if !self.notes.iter().any(|value| value == note) {
            self.notes.push(note.into());
        }
    }
}

fn decode(raw: &[u8]) -> AlpsContents {
    let mut view = AlpsContents {
        peer_bytes: raw.len(),
        fields: vec![],
        notes: vec![],
        payload_base64: STANDARD.encode(raw),
    };
    if raw.is_empty() {
        view.add("Server value", "Empty (ALPS was negotiated)");
    }
    let mut offset = 0;
    let mut frame = 0;
    while offset < raw.len() && !view.full() {
        let data = &raw[offset..];
        if data.len() < 9 {
            view.warn(offset, "Truncated HTTP/2 frame header");
            break;
        }
        let length = usize::from(data[0]) << 16 | usize::from(data[1]) << 8 | usize::from(data[2]);
        let kind = data[3];
        let flags = data[4];
        let stream = u32::from_be_bytes(data[5..9].try_into().expect("frame header")) & 0x7fff_ffff;
        frame += 1;
        let name = match kind {
            4 => "SETTINGS".into(),
            0x89 => "ACCEPT_CH".into(),
            _ => format!("Type 0x{kind:02x}"),
        };
        view.add(
            &format!("Frame {frame} · {name}"),
            format!("{length} payload bytes · flags 0x{flags:02x} · stream {stream}"),
        );
        if length > data.len() - 9 {
            view.warn(offset, "Truncated HTTP/2 frame payload");
            break;
        }
        let payload = &data[9..9 + length];
        if length > 16_384 {
            view.warn(offset, "ALPS frame exceeds the initial HTTP/2 frame size");
        }
        match kind {
            4 => {
                if stream != 0 || flags & 1 != 0 || length % 6 != 0 {
                    view.warn(offset, "Invalid ALPS SETTINGS frame");
                }
                settings(&mut view, payload, offset + 9);
            }
            0x89 => {
                if stream != 0 || flags != 0 {
                    view.warn(offset, "Invalid ALPS ACCEPT_CH frame");
                }
                hints(&mut view, payload, offset + 9);
            }
            _ => {
                if kind <= 9 {
                    view.warn(offset, "This HTTP/2 frame type is forbidden in ALPS");
                }
                if !view.full() {
                    view.add("Frame payload (hex)", preview_hex(payload));
                }
            }
        }
        offset += 9 + length;
    }
    view
}

fn settings(view: &mut AlpsContents, payload: &[u8], offset: usize) {
    for (index, parameter) in payload.as_chunks::<6>().0.iter().enumerate() {
        if view.full() {
            return;
        }
        let id = u16::from_be_bytes(parameter[..2].try_into().expect("setting id"));
        let value = u32::from_be_bytes(parameter[2..].try_into().expect("setting value"));
        let name = match id {
            1 => "Header table size",
            2 => "Server push",
            3 => "Maximum concurrent streams",
            4 => "Initial stream window",
            5 => "Maximum frame size",
            6 => "Maximum header list size",
            8 => "Extended CONNECT",
            _ => "Unknown setting",
        };
        let display = match id {
            2 | 8 if value <= 1 => {
                if value == 1 {
                    "Enabled".into()
                } else {
                    "Disabled".into()
                }
            }
            1 | 4..=6 => format!("{value} bytes"),
            _ => value.to_string(),
        };
        view.add(&format!("{name} (0x{id:04x})"), display);
        if matches!(id, 2 | 8) && value > 1
            || id == 4 && value > 0x7fff_ffff
            || id == 5 && !(16_384..=16_777_215).contains(&value)
        {
            view.warn(offset + index * 6, "Invalid HTTP/2 setting value");
        }
    }
}

fn h3_integer(data: &mut &[u8]) -> Option<u64> {
    let size = 1_usize << (data.first()? >> 6);
    let encoded = data.get(..size)?;
    let mut value = u64::from(encoded[0] & 63);
    for byte in &encoded[1..] {
        value = (value << 8) | u64::from(*byte);
    }
    *data = &data[size..];
    Some(value)
}

fn h3_string<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
    let length = usize::try_from(h3_integer(data)?).ok()?;
    let value = data.get(..length)?;
    *data = &data[length..];
    Some(value)
}

#[allow(clippy::too_many_lines)] // Bounded, read-only projection keeps malformed bytes inspectable.
fn decode_h3(raw: &[u8]) -> AlpsContents {
    let mut view = AlpsContents {
        peer_bytes: raw.len(),
        fields: vec![],
        notes: vec![],
        payload_base64: STANDARD.encode(raw),
    };
    if raw.is_empty() {
        view.add("Server value", "Empty (ALPS was negotiated)");
    }
    let mut data = raw;
    let mut frame = 0;
    let mut settings_seen = false;
    while !data.is_empty() && !view.full() {
        let offset = raw.len() - data.len();
        let Some((kind, length)) = h3_integer(&mut data).zip(h3_integer(&mut data)) else {
            view.warn(offset, "Truncated HTTP/3 frame header");
            break;
        };
        let Some(length) = usize::try_from(length)
            .ok()
            .filter(|length| *length <= data.len())
        else {
            view.warn(offset, "Truncated HTTP/3 frame payload");
            break;
        };
        let (mut payload, remaining) = data.split_at(length);
        data = remaining;
        frame += 1;
        let name = match kind {
            4 => "SETTINGS".into(),
            0x89 => "ACCEPT_CH".into(),
            0xc => "ORIGIN".into(),
            _ => format!("Type 0x{kind:x}"),
        };
        view.add(
            &format!("Frame {frame} · {name}"),
            format!("{length} payload bytes"),
        );
        match kind {
            4 => {
                if settings_seen {
                    view.warn(offset, "Multiple ALPS SETTINGS frames");
                }
                settings_seen = true;
                let mut seen = std::collections::HashSet::new();
                while !payload.is_empty() && !view.full() {
                    let Some((id, value)) = h3_integer(&mut payload).zip(h3_integer(&mut payload))
                    else {
                        view.warn(offset, "Truncated HTTP/3 setting");
                        break;
                    };
                    if !seen.insert(id) {
                        view.warn(offset, "Duplicate HTTP/3 setting");
                    }
                    let label = match id {
                        1 => "QPACK table capacity",
                        6 => "Maximum field section size",
                        7 => "QPACK blocked streams",
                        8 => "Extended CONNECT",
                        0x33 | 0x276 => "HTTP/3 datagrams",
                        _ => "Unknown setting",
                    };
                    view.add(
                        &format!("{label} (0x{id:x})"),
                        if matches!(id, 1 | 6) {
                            format!("{value} bytes")
                        } else {
                            value.to_string()
                        },
                    );
                    if matches!(id, 0 | 2..=5) || matches!(id, 8 | 0x33 | 0x276) && value > 1 {
                        view.warn(offset, "Invalid HTTP/3 setting");
                    }
                }
            }
            0x89 => {
                while !payload.is_empty() && !view.full() {
                    let Some((origin, hints)) =
                        h3_string(&mut payload).zip(h3_string(&mut payload))
                    else {
                        view.warn(offset, "Truncated ACCEPT_CH entry");
                        break;
                    };
                    for (label, bytes) in [
                        ("Client Hint origin", origin),
                        ("Requested Client Hints (ignored)", hints),
                    ] {
                        if view.full() {
                            break;
                        }
                        view.add(label, String::from_utf8_lossy(bytes));
                    }
                }
            }
            _ => {
                if matches!(kind, 0..=3 | 5..=9 | 0xd | 0x4d | 0xf0700 | 0xf0701) {
                    view.warn(offset, "This HTTP/3 frame type is forbidden in ALPS");
                }
                if !view.full() {
                    view.add("Frame payload (hex)", preview_hex(payload));
                }
            }
        }
    }
    view
}

fn string<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
    let length = usize::from(u16::from_be_bytes(data.get(..2)?.try_into().ok()?));
    let value = data.get(2..2 + length)?;
    *data = &data[2 + length..];
    Some(value)
}

fn hints(view: &mut AlpsContents, payload: &[u8], offset: usize) {
    let mut data = payload;
    while !data.is_empty() {
        if view.fields.len() > MAX_FIELDS - 2 {
            view.limited();
            return;
        }
        let entry_offset = offset + payload.len() - data.len();
        let Some((origin, hints)) = string(&mut data).zip(string(&mut data)) else {
            view.warn(entry_offset, "Truncated ACCEPT_CH entry");
            return;
        };
        for (label, bytes) in [
            ("Client Hint origin", origin),
            ("Requested Client Hints (ignored)", hints),
        ] {
            if !bytes.is_ascii() {
                view.warn(
                    entry_offset,
                    "Non-ASCII ACCEPT_CH text; invalid UTF-8 uses replacement characters",
                );
            }
            view.add(
                label,
                if bytes.is_empty() {
                    "(empty)".into()
                } else {
                    String::from_utf8_lossy(bytes).into_owned()
                },
            );
        }
    }
}

fn preview_hex(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes.iter().take(32) {
        let _ = write!(text, "{byte:02x} ");
    }
    if bytes.len() > 32 {
        text.push_str("… (complete bytes in Hex)");
    }
    if bytes.is_empty() {
        text.push_str("(empty)");
    }
    text.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_core::TlsEvidence;

    fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
        let length = u32::try_from(payload.len()).unwrap().to_be_bytes();
        let mut bytes = vec![length[1], length[2], length[3], kind, 0, 0, 0, 0, 0];
        bytes.extend_from_slice(payload);
        bytes
    }
    #[test]
    fn contents_preserve_order_duplicates_unknowns_and_client_hint_text() {
        let mut raw = frame(
            4,
            &[0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 8, 0xfa, 0xfa, 0, 0, 0, 7],
        );
        let origin = b"https://example.test";
        let hints = b"Sec-CH-UA-Platform, Sec-CH-UA-Model";
        let mut entry = u16::try_from(origin.len()).unwrap().to_be_bytes().to_vec();
        entry.extend_from_slice(origin);
        entry.extend_from_slice(&u16::try_from(hints.len()).unwrap().to_be_bytes());
        entry.extend_from_slice(hints);
        raw.extend(frame(0x89, &entry));
        raw.extend(frame(0xfa, &[1, 2, 3]));
        let view = decode(&raw);
        assert_eq!(view.notes, [] as [std::string::String; 0]);
        assert_eq!(STANDARD.decode(&view.payload_base64).unwrap(), raw);
        assert_eq!(
            view.fields
                .iter()
                .filter(|field| field.label == "Header table size (0x0001)")
                .map(|field| field.value.as_str())
                .collect::<Vec<_>>(),
            ["0 bytes", "8 bytes"]
        );
        assert!(
            view.fields
                .iter()
                .any(|field| field.label == "Unknown setting (0xfafa)" && field.value == "7")
        );
        assert!(
            view.fields
                .iter()
                .any(|field| field.label == "Client Hint origin"
                    && field.value == "https://example.test")
        );
        assert!(
            view.fields
                .iter()
                .any(|field| field.label == "Requested Client Hints (ignored)"
                    && field.value == String::from_utf8_lossy(hints))
        );
        assert!(
            view.fields
                .iter()
                .any(|field| field.label == "Frame payload (hex)" && field.value == "01 02 03")
        );
    }

    #[test]
    fn http3_contents_decode_settings_hints_and_malformed_bytes() {
        let mut raw = vec![4, 4, 6, 32, 7, 0, 0x40, 0x89, 24, 20];
        raw.extend_from_slice(b"https://example.test");
        raw.extend_from_slice(&[2, b'U', b'A']);
        let view = decode_h3(&raw);
        assert!(view.notes.is_empty(), "{:?}", view.notes);
        assert!(
            view.fields
                .iter()
                .any(|field| field.label == "Maximum field section size (0x6)"
                    && field.value == "32 bytes")
        );
        assert!(
            view.fields
                .iter()
                .any(|field| field.label == "Requested Client Hints (ignored)"
                    && field.value == "UA")
        );
        assert_eq!(STANDARD.decode(&view.payload_base64).unwrap(), raw);
        for bytes in [
            vec![0x40],
            vec![4, 0, 4, 0],
            vec![4, 2, 6],
            vec![4, 4, 6, 1, 6, 2],
            vec![0x40, 0x89, 1, 1],
        ] {
            let view = decode_h3(&bytes);
            assert!(!view.notes.is_empty(), "{bytes:?}");
            assert_eq!(STANDARD.decode(view.payload_base64).unwrap(), bytes);
        }
        let bytes = [4, 0].repeat(1000);
        let view = decode_h3(&bytes);
        assert_eq!(view.fields.len(), MAX_FIELDS);
        assert_eq!(STANDARD.decode(view.payload_base64).unwrap(), bytes);
    }
    #[test]
    fn malformed_and_large_contents_remain_bounded_with_original_bytes() {
        for raw in [
            vec![0],
            vec![0, 0, 9, 4, 0, 0, 0, 0, 0],
            frame(0x89, &[0, 8, 1]),
            frame(4, &[0, 2, 0, 0, 0, 2]),
            frame(0, &[1]),
        ] {
            let view = decode(&raw);
            assert_ne!(view.notes, [] as [std::string::String; 0]);
            assert_eq!(STANDARD.decode(&view.payload_base64).unwrap(), raw);
        }
        let raw = frame(4, &[0, 1, 0, 0, 0, 1].repeat(1000));
        let view = decode(&raw);
        assert_eq!(view.fields.len(), MAX_FIELDS);
        assert_eq!(view.notes.len(), 1);
        assert_eq!(STANDARD.decode(&view.payload_base64).unwrap(), raw);
        let mut entry = vec![0, 1, 0, 0x0f, 0xa0];
        entry.extend(vec![b'a'; 4000]);
        let view = decode(&frame(0x89, &entry));
        assert!(view.fields[1].display_value.contains("\\u{0000}"));
        assert_eq!(view.fields[2].value.len(), 4000);
        assert!(view.fields[2].display_value.len() < 2100);
    }
    #[test]
    fn empty_negotiated_declined_legacy_and_conflicting_status_are_distinct() {
        for actual in [None, Some(false), Some(true)] {
            let transport = TransportObservation {
                tls: Some(TlsEvidence {
                    http2_alps: actual.map(|negotiated| Http2AlpsEvidence {
                        negotiated,
                        peer_settings: vec![].into(),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let mut view = CertificateConnection {
                id: String::new(),
                title: String::new(),
                fields: vec![],
                summary: String::new(),
                policy_fields: vec![],
                diagnostic_fields: vec![],
                alps: None,
                server_chain: vec![],
                client_chain: vec![],
                errors: vec![],
            };
            organize(&mut view, &transport);
            assert_eq!(
                view.fields[0].value,
                match actual {
                    None => "Unavailable",
                    Some(false) => "Not negotiated",
                    Some(true) => "Negotiated (0 peer bytes)",
                }
            );
            assert_eq!(view.alps.is_some(), actual == Some(true));
            let mut legacy = transport.clone();
            legacy
                .tls
                .as_mut()
                .unwrap()
                .details
                .insert("HTTP/2 ALPS".into(), "Negotiated (188 peer bytes)".into());
            organize(&mut view, &legacy);
            if actual.is_none() {
                assert!(
                    view.diagnostic_fields
                        .iter()
                        .any(|field| field.label == "ALPS contents")
                );
            } else {
                assert!(view.errors.iter().any(|error| error.contains("disagree")));
            }
        }
    }
}
