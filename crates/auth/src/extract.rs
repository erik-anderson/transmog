//! Source extraction preserves ordering, duplicates, empty values, and original encodings.
use crate::{Context, Finding, Input, MAX_CANDIDATE, Source, http};

pub(crate) struct Pair {
    pub name: String,
    pub value: String,
    pub original: String,
    pub delimiter: bool,
    pub source: Source,
}

fn standard_header(name: &str) -> bool {
    [
        "authorization",
        "proxy-authorization",
        "www-authenticate",
        "proxy-authenticate",
        "authentication-info",
        "proxy-authentication-info",
        "location",
    ]
    .iter()
    .any(|standard| name.eq_ignore_ascii_case(standard))
}

/// Whether a header can supply auth evidence, including custom token candidates.
/// The name is only a hint; custom values still require recognizable JOSE structure.
pub fn is_candidate_header(name: &str) -> bool {
    standard_header(name)
        || [b"auth".as_slice(), b"access", b"token"]
            .iter()
            .any(|part| {
                name.as_bytes()
                    .windows(part.len())
                    .any(|window| window.eq_ignore_ascii_case(part))
            })
}
fn percent(raw: &str, form: bool) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                let digits = bytes.get(at + 1..at + 3)?;
                let hi = char::from(digits[0]).to_digit(16)?;
                let lo = char::from(digits[1]).to_digit(16)?;
                decoded.push(u8::try_from(hi * 16 + lo).ok()?);
                at += 3;
            }
            b'+' if form => {
                decoded.push(b' ');
                at += 1;
            }
            b => {
                decoded.push(b);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}
pub(crate) fn pairs(context: &mut Context, text: &str, source: &Source, form: bool) -> Vec<Pair> {
    let mut offset = 0;
    let mut result = vec![];
    for (occurrence, part) in text.split('&').enumerate().take(2049) {
        if occurrence == 2048 {
            context.coverage(
                &source.kind,
                "limited",
                "Parameter count exceeded the bounded scan; remaining fields were not inspected.",
            );
            break;
        }
        if !context.step() {
            context.limited();
            break;
        }
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        if let (Some(name), Some(value)) = (
            percent(key, form),
            percent(
                value,
                form && (source.kind == "body" || key != "SAMLRequest" && key != "SAMLResponse"),
            ),
        ) {
            let mut loc = source.clone();
            loc.occurrence = occurrence;
            loc.offset = Some(offset);
            loc.length = Some(part.len());
            loc.transforms.push(
                if form {
                    "Form URL decoding"
                } else {
                    "URL percent decoding"
                }
                .into(),
            );
            result.push(Pair {
                name,
                value,
                original: part.into(),
                delimiter: part.contains('='),
                source: loc,
            });
        } else {
            context.coverage(
                &source.kind,
                "incomplete",
                "A parameter had malformed percent encoding or unsupported text bytes.",
            );
        }
        offset += part.len() + 1;
    }
    result
}
pub(crate) fn parameters(context: &mut Context, items: &[Pair]) {
    if let Some(finding) = crate::oauth::decode(context, items) {
        context.push(finding);
    }
    for pair in items
        .iter()
        .filter(|p| ["SAMLRequest", "SAMLResponse", "SAMLart"].contains(&p.name.as_str()))
    {
        let finding = crate::saml::parameter(context, pair, items);
        context.push(finding);
    }
    if crate::oauth::role(items).is_some() {
        return;
    }
    for pair in items.iter().filter(|p| {
        ["access_token", "refresh_token", "client_assertion"].contains(&p.name.as_str())
    }) {
        let mut finding = Finding::new("Auth parameter", &pair.name, pair.source.clone());
        context.field(
            &mut finding,
            &pair.name,
            &pair.value,
            true,
            "Observed value; no signature verification, redemption or network lookup.",
        );
        context.field(
            &mut finding,
            "Original encoded field",
            &pair.original,
            true,
            if pair.delimiter {
                "The original '=' delimiter was present."
            } else {
                "The parameter had no '=' delimiter."
            },
        );
        finding.status = "opaque".into();
        if let Some(child) = crate::jose::decode(context, &pair.value, pair.source.clone()) {
            finding.children.push(child);
        }
        context.push(finding);
    }
}
fn url(context: &mut Context, value: &str, source: Source, fragment: bool) {
    if let Some((_, query)) = value.split('#').next().unwrap_or("").split_once('?') {
        let mut source = source.clone();
        if fragment {
            source.kind = "redirect query".into();
            source.label.push_str(" query");
        }
        let items = pairs(context, query, &source, true);
        parameters(context, &items);
    }
    if fragment && let Some((_, value)) = value.split_once('#') {
        let mut source = source;
        source.kind = "redirect fragment".into();
        source.label.push_str(" fragment (captured Location)");
        let items = pairs(context, value, &source, true);
        parameters(context, &items);
    }
}
pub(crate) fn message(context: &mut Context, input: Input) {
    let mut headers_complete = true;
    for (index, header) in input.headers.into_iter().enumerate() {
        if !context.step() {
            headers_complete = false;
            break;
        }
        if header.limited || header.name.len() > MAX_CANDIDATE {
            headers_complete = false;
            let mut finding = Finding::new(
                "Auth source budget",
                "retained header coverage",
                Source {
                    kind: "header".into(),
                    label: format!("Retained header field {}", index + 1),
                    occurrence: index,
                    header_index: Some(index),
                    ..Source::default()
                },
            );
            finding.recognition = "candidate".into();
            finding.issue("limited", "Header bytes or remaining fields exceeded native source admission limits; other admitted sources remain inspectable.");
            context.push(finding);
            continue;
        }
        let standard = standard_header(&header.name);
        let custom_token = !standard && is_candidate_header(&header.name);
        if !standard && !custom_token {
            continue;
        }
        if !context.charge(header.name.len()) {
            headers_complete = false;
            context.coverage(
                "headers",
                "limited",
                "An auth header name exceeded the inspection budget.",
            );
            continue;
        }
        let key = header.name.to_ascii_lowercase();
        let source = Source {
            kind: "header".into(),
            label: format!("{} · field {}", crate::clip(&header.name, 128), index + 1),
            occurrence: index,
            header_index: Some(index),
            offset: Some(0),
            length: (!header.redacted).then_some(header.value.len()),
            transforms: vec![],
        };
        if header.redacted {
            let mut finding = Finding::new("Redacted auth header", "retained redaction", source);
            finding.status = "redacted".into();
            finding
                .diagnostics
                .push("Captured value was redacted and cannot be revealed or decoded.".into());
            context.push(finding);
            continue;
        }
        if header.value.len() > MAX_CANDIDATE || !context.charge(header.value.len()) {
            headers_complete = false;
            context.coverage(
                "headers",
                "limited",
                "An auth header exceeded the encoded candidate budget.",
            );
            continue;
        }
        let Ok(value) = std::str::from_utf8(&header.value) else {
            if custom_token {
                continue;
            }
            let mut finding = Finding::new("HTTP auth", "header", source);
            finding.issue(
                "unsupported",
                "Header bytes are not supported UTF-8/ASCII text.",
            );
            context.push(finding);
            continue;
        };
        if custom_token {
            let (token, offset) = crate::jose::candidate(value);
            let mut source = source;
            source.offset = Some(offset);
            source.length = Some(token.len());
            if let Some(mut finding) = crate::jose::decode(context, token, source) {
                finding.role = "custom header token".into();
                if finding.status == "limited" {
                    headers_complete = false;
                }
                context.push(finding);
            }
            continue;
        }
        if key == "location" {
            url(context, value, source, true);
            continue;
        }
        let role = if key.starts_with("proxy-") {
            if key.ends_with("authorization") {
                "proxy credential"
            } else {
                "proxy challenge"
            }
        } else if key == "authorization" {
            "origin credential"
        } else {
            "origin challenge"
        };
        let value = if key.ends_with("authentication-info") {
            format!("Digest {value}")
        } else {
            value.into()
        };
        let envelopes = http::parse(&value);
        if envelopes.len() == 64 {
            headers_complete = false;
            context.limited();
        }
        for item in envelopes {
            let mut finding = http::decode(context, item, source.clone(), role);
            if key.ends_with("authentication-info") {
                if let Ok(original) = std::str::from_utf8(&header.value) {
                    context.token(&mut finding, original, "digest-parse");
                }
                finding.source.offset = None;
                finding.source.length = None;
                finding.source.transforms.push(
                    "Authentication-Info parameter grammar (no scheme prefix in the original)"
                        .into(),
                );
            }
            if finding.status == "limited" {
                headers_complete = false;
            }
            context.push(finding);
        }
    }
    headers_complete &= context.result.diagnostics.is_empty();
    context.coverage(
        "headers",
        if headers_complete {
            "complete"
        } else {
            "limited"
        },
        "All retained header occurrences are considered independently of display pagination.",
    );
    if let Some(target) = input.target {
        if target.len() <= MAX_CANDIDATE && context.charge(target.len()) {
            url(
                context,
                &target,
                Source {
                    kind: "query".into(),
                    label: "Retained request target query".into(),
                    ..Source::default()
                },
                false,
            );
            context.coverage(
                "query",
                "complete",
                "Only the transmitted query is available; browser fragments are absent.",
            );
        } else {
            context.coverage(
                "query",
                "limited",
                "Request target exceeds the input budget.",
            );
        }
    }
    if let Some(body) = input.body {
        if body.bytes.len() > 4 * 1024 * 1024 || !context.charge(body.bytes.len()) {
            context.coverage(
                "body",
                "limited",
                "Decoded body exceeds the aggregate scan budget.",
            );
            return;
        }
        crate::body::scan(context, body);
        if context.result.findings.iter().any(|finding| {
            finding.format == "OAuth / OpenID Connect" && finding.role == "token request"
        }) {
            for finding in &mut context.result.findings {
                if finding.format == "Basic" && finding.role == "origin credential" {
                    finding.role = "Basic credential in OAuth token-request context".into();
                    finding.diagnostics.push("Client authentication is a contextual interpretation; no credentials are validated or sent.".into());
                }
            }
        }
    } else {
        context.coverage(
            "body",
            &input.body_status,
            "Selected-message bodies are read only for an active auth inspection.",
        );
    }
}
