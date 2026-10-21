//! RFC 9110 challenge grammar. Commas inside quoted values are not challenge boundaries.
use crate::{Context, Finding, Source};
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub(crate) struct Envelope {
    pub scheme: String,
    pub token: Option<String>,
    pub params: Vec<(String, String)>,
    pub malformed: bool,
    pub limited: bool,
    pub start: usize,
    pub end: usize,
}
fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}
fn space(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).is_some_and(|b| b" \t".contains(b)) {
        *at += 1;
    }
}
fn word(bytes: &[u8], at: &mut usize) -> String {
    let start = *at;
    while bytes.get(*at).is_some_and(|b| token(*b)) {
        *at += 1;
    }
    String::from_utf8_lossy(&bytes[start..*at]).into_owned()
}
pub(crate) fn parse(value: &str) -> Vec<Envelope> {
    let bytes = value.as_bytes();
    let mut at = 0;
    let mut result = vec![];
    while at < bytes.len() && result.len() < 64 {
        space(bytes, &mut at);
        if bytes.get(at) == Some(&b',') {
            at += 1;
            continue;
        }
        let start = at;
        let scheme = word(bytes, &mut at);
        if scheme.is_empty() {
            break;
        }
        let separated = bytes.get(at).is_none_or(|b| b" \t,".contains(b));
        space(bytes, &mut at);
        let mut item = Envelope {
            scheme,
            token: None,
            params: vec![],
            malformed: !separated,
            limited: false,
            start,
            end: at,
        };
        if at < bytes.len() && bytes[at] != b',' {
            let mark = at;
            let mut probe = at;
            word(bytes, &mut probe);
            space(bytes, &mut probe);
            let parameter = bytes.get(probe) == Some(&b'=')
                && bytes
                    .get(probe + 1)
                    .is_some_and(|b| *b != b'=' && *b != b',')
                && !bytes[probe + 1..]
                    .iter()
                    .all(|b| *b == b'=' || b.is_ascii_whitespace());
            if parameter {
                loop {
                    let key = word(bytes, &mut at);
                    space(bytes, &mut at);
                    if key.is_empty() || bytes.get(at) != Some(&b'=') {
                        item.malformed = true;
                        break;
                    }
                    at += 1;
                    space(bytes, &mut at);
                    let val;
                    if bytes.get(at) == Some(&b'"') {
                        at += 1;
                        let mut closed = false;
                        let mut quoted = Vec::new();
                        while at < bytes.len() {
                            let b = bytes[at];
                            at += 1;
                            if b == b'"' {
                                closed = true;
                                break;
                            }
                            if b == b'\\' {
                                if at == bytes.len() {
                                    break;
                                }
                                quoted.push(bytes[at]);
                                at += 1;
                            } else {
                                quoted.push(b);
                            }
                        }
                        item.malformed |= !closed;
                        val = String::from_utf8_lossy(&quoted).into_owned();
                    } else {
                        val = word(bytes, &mut at);
                        if val.is_empty() {
                            item.malformed = true;
                        }
                    }
                    item.params.push((key, val));
                    space(bytes, &mut at);
                    if item.params.len() >= 256 && at < bytes.len() {
                        item.limited = true;
                        at = bytes.len();
                        break;
                    }
                    if bytes.get(at) != Some(&b',') {
                        break;
                    }
                    let comma = at;
                    probe = at + 1;
                    space(bytes, &mut probe);
                    word(bytes, &mut probe);
                    space(bytes, &mut probe);
                    if bytes.get(probe) != Some(&b'=') {
                        at = comma;
                        break;
                    }
                    at = comma + 1;
                    space(bytes, &mut at);
                }
            } else {
                while at < bytes.len() && bytes[at] != b',' {
                    at += 1;
                }
                let raw = value[mark..at].trim();
                let core = raw.trim_end_matches('=');
                item.malformed |= core.is_empty()
                    || !core
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~+/".contains(&b));
                item.token = Some(raw.into());
            }
        }
        item.end = at;
        if bytes.get(at).is_some_and(|b| *b != b',') {
            item.malformed = true;
            while at < bytes.len() && bytes[at] != b',' {
                at += 1;
            }
        }
        result.push(item);
        if at == start {
            break;
        }
    }
    result
}
pub(crate) fn decode(
    context: &mut Context,
    item: Envelope,
    mut source: Source,
    role: &str,
) -> Finding {
    source.offset = Some(item.start);
    source.length = Some(item.end.saturating_sub(item.start));
    let name = match item.scheme.to_ascii_lowercase().as_str() {
        "basic" => "Basic",
        "digest" => "Digest",
        "bearer" => "Bearer",
        "ntlm" => "NTLM",
        "negotiate" => "Negotiate",
        _ => "Unknown HTTP auth scheme",
    };
    let mut finding = Finding::new(name, role, source);
    if item.token.is_none() && item.params.is_empty() && role.ends_with("credential") {
        finding.issue(
            "malformed",
            "A credential-bearing header advertised a scheme without credentials.",
        );
    }
    context.field(
        &mut finding,
        "Scheme",
        &item.scheme,
        false,
        "Observed scheme; names are case-insensitive.",
    );
    if item.malformed {
        finding.issue("malformed", "Malformed HTTP authentication envelope.");
    }
    if item.limited {
        finding.issue(
            "limited",
            "Authentication parameter budget reached; remaining envelope data was not interpreted.",
        );
    }
    for (index, (key, value)) in item.params.iter().enumerate() {
        if item.params[..index]
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(key))
        {
            finding.issue(
                "incomplete",
                "Duplicate authentication parameters; no authoritative value selected.",
            );
        }
        let public = [
            "realm",
            "username",
            "userhash",
            "algorithm",
            "uri",
            "qop",
            "nc",
            "stale",
            "charset",
            "scope",
            "error",
        ]
        .iter()
        .any(|k| key.eq_ignore_ascii_case(k));
        context.field(
            &mut finding,
            key,
            value,
            !public,
            if key.eq_ignore_ascii_case("response") {
                "Digest response hash; not a password. Not validated."
            } else {
                "Observed authentication parameter."
            },
        );
    }
    if let Some(raw) = item.token {
        context.field(
            &mut finding,
            "Original credential",
            &raw,
            true,
            "Explicit copy/reveal includes credential material.",
        );
        if name == "Basic" && role.ends_with("credential") {
            match STANDARD.decode(&raw) {
                Ok(bytes) => {
                    finding.source.transforms.push("Base64".into());
                    if let Some(colon) = bytes.iter().position(|b| *b == b':') {
                        context.field(&mut finding, "Username", String::from_utf8_lossy(&bytes[..colon]), false, "Split at the first colon; non-ASCII charset may be ambiguous without an advertised UTF-8 challenge.");
                        context.field(&mut finding, "Password", String::from_utf8_lossy(&bytes[colon + 1..]), true, "Captured bytes are also retained in Original credential; no password validation.");
                        context.field(
                            &mut finding,
                            "Decoded bytes (Base64)",
                            STANDARD.encode(bytes),
                            true,
                            "Lossless decoded byte representation, including non-UTF-8 text.",
                        );
                    } else {
                        finding.issue(
                            "malformed",
                            "Basic credentials have no username/password delimiter.",
                        );
                    }
                }
                Err(_) => finding.issue("malformed", "Invalid Base64 in Basic credentials."),
            }
        } else {
            finding.status = "opaque".into();
            if name == "NTLM" || name == "Negotiate" {
                match STANDARD.decode(&raw) {
                    Ok(bytes) => {
                        if context.charge(bytes.len()) {
                            let mut source = finding.source.clone();
                            source.transforms.push("Base64".into());
                            let child =
                                crate::windows::decode(context, &bytes, source, name == "NTLM");
                            finding.children.push(child);
                        } else {
                            finding.status = "limited".into();
                        }
                    }
                    Err(_) => finding.issue(
                        "malformed",
                        "Invalid Base64 in Windows authentication token.",
                    ),
                }
            }
            if name == "Bearer"
                && let Some(inner) = crate::jose::decode(context, &raw, finding.source.clone())
            {
                finding.children.push(inner);
            }
        }
    }
    finding
}
