//! Compact JOSE parsing only; keys, signatures and encrypted contents are never resolved.
use crate::{Context, Finding, MAX_CANDIDATE, Source, json};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

pub(crate) fn decode(context: &mut Context, raw: &str, mut source: Source) -> Option<Finding> {
    if raw.len() > MAX_CANDIDATE {
        return None;
    }
    let parts: Vec<_> = raw.split('.').take(6).collect();
    if parts.len() != 3 && parts.len() != 5 {
        return None;
    }
    let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).ok()?;
    if !context.charge(header_bytes.len()) {
        return None;
    }
    let header = json::parse(&header_bytes)?;
    header.members()?;
    let alg = header.get("alg").and_then(json::Node::string);
    // Duplicate alg remains a recognized ambiguous JOSE object, rather than selecting the last member.
    if alg.is_none() && !header.members()?.iter().any(|(k, _)| k == "alg") {
        return None;
    }
    source
        .transforms
        .push("Base64url (compact JOSE; no padding)".into());
    let mut finding = Finding::new(
        if parts.len() == 5 { "JWE" } else { "JWS" },
        "compact token",
        source,
    );
    finding.verification = "Not checked".into();
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
    context.field(
        &mut finding,
        "Decoded JOSE header",
        String::from_utf8_lossy(&header_bytes),
        true,
        "Unverified captured JSON; embedded keys and URLs never trigger requests.",
    );
    json::fields(context, &mut finding, &header, "JOSE", false);
    if header.duplicate() {
        finding.issue(
            "incomplete",
            "Duplicate JOSE members are ambiguous; no authoritative member selected.",
        );
    }
    if header.get("crit").is_some() {
        finding.issue(
            "unsupported",
            "Critical JOSE extensions are exposed but not implemented or validated.",
        );
    }
    if header.get("b64").and_then(json::Node::text) == Some("false") {
        finding.issue(
            "unsupported",
            "Unencoded JWS payload serialization is not supported.",
        );
        return Some(finding);
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
    if let Some(claims) = json::parse(&payload) {
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
                finding.issue(
                    "incomplete",
                    "Duplicate payload members are retained; claims are ambiguous.",
                );
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
    } else {
        context.field(&mut finding, "Payload text", String::from_utf8_lossy(&payload), true, "JWS payload is not a supported JSON claims set; original bytes are available separately.");
    }
    Some(finding)
}
