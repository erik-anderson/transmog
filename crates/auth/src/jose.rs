//! Compact JOSE parsing only; keys, signatures and encrypted contents are never resolved.
use crate::{Context, Finding, MAX_CANDIDATE, Source, json};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

/// Accept a complete token or a Bearer header value, retaining its source offset.
pub(crate) fn candidate(raw: &str) -> (&str, usize) {
    let trimmed = raw.trim_ascii();
    let token = trimmed
        .split_once([' ', '\t'])
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map_or(trimmed, |(_, token)| token.trim_ascii_start());
    (
        token,
        raw.len() - raw.trim_ascii_start().len() + trimmed.len() - token.len(),
    )
}

fn payload_encoding(header: &json::Node) -> Option<bool> {
    if !header.members()?.iter().any(|(name, _)| name == "b64") {
        return Some(true);
    }
    match header.get("b64")? {
        json::Node::Scalar(value) if value == "true" => Some(true),
        json::Node::Scalar(value) if value == "false" => Some(false),
        _ => None,
    }
}

/// Decode a complete compact JWT into readable JSON, without verifying it.
/// Accepts surrounding ASCII whitespace and an optional Bearer prefix.
/// Duplicate JSON members, member order and numeric spellings are preserved.
///
/// # Errors
/// Returns content-free errors for malformed, encrypted, unsupported or oversized input.
pub fn decode_jwt(raw: &str) -> Result<String, &'static str> {
    crate::scratch::jwt(raw, &std::sync::atomic::AtomicBool::new(false))
}

pub(crate) fn decode(context: &mut Context, raw: &str, source: Source) -> Option<Finding> {
    let finding = decode_compact(context, raw, source)?;
    if finding.status == "limited" {
        let source = if finding.source.header_index.is_some() || finding.source.kind == "header" {
            "headers"
        } else {
            &finding.source.kind
        };
        if !source.is_empty() {
            context.coverage(source, "limited", "A compact JOSE token exceeded an inspection byte, work, JSON depth or field limit.");
        }
    }
    Some(finding)
}

fn decode_compact(context: &mut Context, raw: &str, mut source: Source) -> Option<Finding> {
    if raw.len() > MAX_CANDIDATE {
        return None;
    }
    let parts: Vec<_> = raw.split('.').take(6).collect();
    if parts.len() != 3 && parts.len() != 5 {
        return None;
    }
    let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).ok()?;
    source
        .transforms
        .push("Base64url (compact JOSE; no padding)".into());
    let mut finding = Finding::new(
        if parts.len() == 5 { "JWE" } else { "JWS" },
        "compact token",
        source,
    );
    finding.verification = "Not checked".into();
    if !context.charge(header_bytes.len()) {
        finding.recognition = "candidate".into();
        finding.issue(
            "limited",
            "JOSE header decoding exceeded the inspection budget.",
        );
        return Some(finding);
    }
    let header = match json::parse_bounded(&header_bytes) {
        Ok(header) => header,
        Err(json::ParseError::Malformed) => return None,
        Err(json::ParseError::Limited) => {
            finding.recognition = "candidate".into();
            finding.issue(
                "limited",
                "JOSE header exceeded the JSON depth or field limit.",
            );
            return Some(finding);
        }
    };
    header.members()?;
    if !context.charge(header_bytes.len()) {
        finding.issue(
            "limited",
            "JOSE document exceeded the retained material budget.",
        );
        return Some(finding);
    }
    finding.content.push(("header".into(), header.clone()));
    let alg = header.get("alg").and_then(json::Node::string);
    // Duplicate alg remains a recognized ambiguous JOSE object, rather than selecting the last member.
    if alg.is_none() && !header.members()?.iter().any(|(k, _)| k == "alg") {
        return None;
    }
    if alg.is_none() {
        finding.issue(
            "malformed",
            "The JOSE alg member is missing, duplicated or not a string.",
        );
    }
    context.field(
        &mut finding,
        "Compact token",
        raw,
        true,
        "Raw token includes credentials and cryptographic material; explicit reveal required.",
    );
    context.token(
        &mut finding,
        raw,
        if parts.len() == 5 {
            "jwe-inspect"
        } else {
            "jws-decode"
        },
    );
    context.field(
        &mut finding,
        "Decoded JOSE header",
        String::from_utf8_lossy(&header_bytes),
        true,
        "Unverified captured JSON; embedded keys and URLs never trigger requests.",
    );
    json::fields(context, &mut finding, &header, "JOSE", false);
    if header.duplicate() {
        finding
            .ambiguity("Duplicate JOSE members are ambiguous; no authoritative member selected.");
    }
    if header.get("crit").is_some() {
        finding.issue(
            "unsupported",
            "Critical JOSE extensions are exposed but not implemented or validated.",
        );
    }
    if parts.len() == 5 {
        if header.get("enc").and_then(json::Node::string).is_none() {
            finding.issue(
                "malformed",
                "Compact JWE protected header has no unambiguous enc member.",
            );
        }
        for (name, part) in [
            "Encrypted key",
            "Initialization vector",
            "Ciphertext",
            "Authentication tag",
        ]
        .iter()
        .zip(&parts[1..])
        {
            match URL_SAFE_NO_PAD.decode(part) {
                Ok(bytes) => {
                    if ["Initialization vector", "Authentication tag"].contains(name)
                        && bytes.is_empty()
                    {
                        finding.issue(
                            "malformed",
                            "Compact JWE has an empty initialization vector or authentication tag.",
                        );
                    }
                    context.field(
                        &mut finding,
                        &format!("{name} length"),
                        bytes.len().to_string(),
                        false,
                        "Exposed component length; encrypted contents cannot be read.",
                    );
                }
                Err(_) => finding.issue("malformed", "Invalid compact JWE Base64url component."),
            }
        }
        if finding.status == "decoded" {
            finding.status = "encrypted".into();
        }
        finding.diagnostics.push("Claims, identities and expiry inside the encrypted payload are unavailable. No decryption or authentication is performed.".into());
        return Some(finding);
    }
    match payload_encoding(&header) {
        Some(true) => {}
        Some(false) => {
            finding.issue(
                "unsupported",
                "Unencoded JWS payload serialization is not supported.",
            );
            return Some(finding);
        }
        None => {
            finding.issue(
                "malformed",
                "The JOSE b64 member must be one unambiguous JSON boolean.",
            );
            return Some(finding);
        }
    }
    let Ok(payload) = URL_SAFE_NO_PAD.decode(parts[1]) else {
        finding.issue("malformed", "Invalid JWS payload Base64url.");
        return Some(finding);
    };
    if !context.charge(payload.len()) {
        finding.status = "limited".into();
        return Some(finding);
    }
    match URL_SAFE_NO_PAD.decode(parts[2]) {
        Ok(signature) => {
            if signature.is_empty() && alg != Some("none") {
                finding.issue(
                    "incomplete",
                    "An empty signature was observed for an algorithm other than none.",
                );
            }
            if !signature.is_empty() && alg == Some("none") {
                finding.issue(
                    "malformed",
                    "An unsecured JWT with alg none must have an empty signature.",
                );
            }
            context.field(
                &mut finding,
                "Signature length",
                signature.len().to_string(),
                false,
                "Signature not checked, including alg none.",
            );
        }
        Err(_) => finding.issue("malformed", "Invalid JWS signature Base64url."),
    }
    context.field(
        &mut finding,
        "Decoded payload bytes (Base64)",
        base64::engine::general_purpose::STANDARD.encode(&payload),
        true,
        "Lossless payload, gated because it can contain secrets or personal data.",
    );
    finding
        .content
        .push(("signature".into(), json::Node::Text(parts[2].into())));
    finding.content.push((
        "payloadBytesBase64".into(),
        json::Node::Text(base64::engine::general_purpose::STANDARD.encode(&payload)),
    ));
    let claims = match json::parse_bounded(&payload) {
        Ok(claims) => Some(claims),
        Err(json::ParseError::Malformed) => None,
        Err(json::ParseError::Limited) => {
            finding.issue(
                "limited",
                "JWS payload exceeded the JSON depth or field limit.",
            );
            return Some(finding);
        }
    };
    if let Some(claims) = claims {
        if !context.charge(payload.len()) {
            finding.issue(
                "limited",
                "JOSE document exceeded the retained material budget.",
            );
            return Some(finding);
        }
        finding.content.push(("payload".into(), claims.clone()));
        if claims.members().is_some() {
            if header
                .get("typ")
                .and_then(json::Node::text)
                .is_some_and(|v| v.eq_ignore_ascii_case("JWT"))
                || ["iss", "sub", "aud", "exp", "iat", "nbf"]
                    .iter()
                    .any(|key| claims.get(key).is_some())
            {
                finding.format = "JWT / JWS".into();
            }
            if claims.duplicate() {
                finding.ambiguity("Duplicate payload members are retained; claims are ambiguous.");
            }
            context.field(
                &mut finding,
                "Decoded JSON payload",
                String::from_utf8_lossy(&payload),
                true,
                "Captured claims remain unverified and can include personal data.",
            );
            json::fields(context, &mut finding, &claims, "", false);
        } else {
            context.field(
                &mut finding,
                "Payload text",
                String::from_utf8_lossy(&payload),
                true,
                "JSON payload is not a claims object.",
            );
        }
    } else if let Ok(text) = std::str::from_utf8(&payload) {
        context.field(&mut finding, "Payload text", text, true, "JWS payload is not a supported JSON claims set; original bytes are available separately.");
    } else {
        finding.diagnostics.push("JWS payload is binary or not UTF-8; its lossless Base64 bytes are retained without guessing a character set.".into());
    }
    let operation = crate::conversion_for_format(&finding.format).unwrap_or("jws-decode");
    if finding.converter_operation.is_some() {
        finding.converter_operation = Some(operation);
    }
    Some(finding)
}
